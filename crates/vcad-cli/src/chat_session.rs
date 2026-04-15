//! Async bridge between the sync TUI loop and `vcad_chat::Client`.
//!
//! The main TUI loop is a 16 ms `event::poll` cycle, so it can't `.await`
//! a reqwest stream directly. This module spawns a background thread that
//! owns a private tokio runtime, runs `Client::stream` to completion, and
//! forwards normalized [`ChatUpdate`] values back over a
//! `std::sync::mpsc::Sender`. The main loop drains those updates each
//! frame via [`drain_chat_events`] and applies them to [`App`]'s
//! conversation state + chat panel lines.
//!
//! The plumbing here is deliberately boring — no shared mutex on the
//! document. The background thread only builds HTTP requests and parses
//! events; every document mutation happens on the main thread in
//! [`drain_chat_events`] so we never fight the kernel over `&mut Document`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::{json, Value};

use vcad_chat::{
    anthropic_tools, build_system_prompt, execute_crud, load_token, stream::ChatRequest,
    ChatContext, ChatError, ChatEvent, ChatMessage, Client, MessageContent, MessageRole,
    SelectedPart, SelectionInfo,
};

use crate::app::{App, LogLevel};
use crate::ui::chat::ChatLineKind;

/// Default chat endpoint. Override via `VCAD_CHAT_ENDPOINT`.
const DEFAULT_ENDPOINT: &str = "https://vcad.io/api/chat";

/// A single event delivered from the background thread to the main loop.
#[derive(Debug)]
pub enum ChatUpdate {
    /// Partial assistant text — append to the streaming assistant line.
    Text(String),
    /// A tool call fully assembled from `tool_start` + `tool_delta*` + `block_stop`.
    ToolCall {
        /// Unique id from the server. Threaded into `ToolUseRecord` and
        /// emitted as the `id` / `tool_use_id` on both halves of the
        /// next request's tool_use + tool_result content blocks.
        id: String,
        name: String,
        args: Value,
    },
    /// Stream finished cleanly.
    Done,
    /// Fatal stream error. The caller renders a friendly message via
    /// [`friendly_error_text`] so the user sees actionable guidance
    /// instead of a raw error body.
    Error(ChatErrorKind),
}

/// Typed error kind delivered over the stream channel. Preserves enough
/// information for [`friendly_error_text`] to pick the right message
/// without embedding UI strings in the background task.
///
/// `body` fields are preserved for future diagnostic rendering (details
/// pane, debug panel) but aren't surfaced by the current friendly
/// formatter — they get `#[allow(dead_code)]` so clippy doesn't complain.
#[derive(Debug, Clone)]
pub enum ChatErrorKind {
    /// Server returned 429. Anonymous quota hit or signed-in quota exceeded.
    RateLimited {
        #[allow(dead_code)]
        body: String,
    },
    /// Non-2xx, non-429 HTTP response — most commonly 404 (endpoint
    /// missing) or 500 (upstream Anthropic failure).
    Http {
        status: u16,
        #[allow(dead_code)]
        body: String,
    },
    /// reqwest or tokio error before/during the stream — connection
    /// refused, DNS failure, TLS error, etc.
    Network(String),
    /// The stream parser rejected the payload — shouldn't happen unless
    /// the server protocol changes.
    Parse(String),
}

/// One tool invocation from a single assistant turn — recorded as it
/// happens so we can build the tool_use + tool_result content blocks
/// that chain the next request.
#[derive(Debug, Clone)]
pub struct ToolUseRecord {
    pub id: String,
    pub name: String,
    pub input: Value,
    /// Execution summary — filled immediately after `execute_crud`.
    pub result: String,
    /// True when the execution succeeded. Anthropic's `tool_result` has
    /// an optional `is_error` flag we set when this is false.
    pub is_error: bool,
}

/// Persistent chat state owned by [`App`].
#[derive(Debug, Default)]
pub struct ChatSession {
    /// Full conversation history — sent on every new request so the model
    /// has context for multi-turn interaction.
    pub messages: Vec<ChatMessage>,
    /// Channel for updates from the in-flight request thread, or `None`
    /// when nothing is streaming.
    pub event_rx: Option<Receiver<ChatUpdate>>,
    /// Accumulating assistant response text for the current turn.
    pub assistant_buffer: String,
    /// Tool calls the assistant has made in the current turn, each
    /// already executed — composed into the next request as tool_use +
    /// tool_result content blocks when the turn completes.
    pub pending_tools: Vec<ToolUseRecord>,
    /// True while a request is in flight; blocks new sends and the
    /// assistant row renders with a spinner.
    pub in_flight: bool,
    /// Shared abort flag — set to true by [`ChatSession::abort`] on the
    /// main thread, polled by the bg request loop before each event.
    /// When true the bg thread drops its stream and emits a `Done` so
    /// the drain path finalizes whatever partial text was received.
    abort_flag: Option<Arc<AtomicBool>>,
}

impl ChatSession {
    /// True when a request is streaming or tool-execution is pending.
    pub fn is_busy(&self) -> bool {
        self.in_flight
    }

    /// Signal the in-flight request to stop ASAP. Safe to call when
    /// nothing is streaming (no-op).
    pub fn abort(&self) {
        if let Some(flag) = &self.abort_flag {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Start a new chat turn — user message already appended to
/// `session.messages`. Spawns the background thread that runs the HTTP
/// request and begins forwarding events.
pub fn start_chat_turn(app: &mut App) -> Result<()> {
    if app.chat_session.is_busy() {
        app.set_status("Chat: a request is already in flight");
        return Ok(());
    }

    // Build the ChatRequest off the current document + selection so the
    // model sees what the user sees.
    let parts = vcad_chat::parts_from_document(&app.document);
    let selection = selection_from_app(app);
    let system_prompt = build_system_prompt(&parts, &selection);
    let context = ChatContext {
        selected_parts: selection_to_selected_parts(&selection),
    };

    let request = ChatRequest {
        messages: app.chat_session.messages.clone(),
        context,
        tools: anthropic_tools(),
        system_prompt,
    };

    // Auth token — anonymous is allowed (3 messages/day via IP), but if a
    // token exists we use it so the per-user quota applies.
    let bearer = load_token()
        .ok()
        .flatten()
        .map(|t| t.access_token);

    let endpoint =
        std::env::var("VCAD_CHAT_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    let (tx, rx) = mpsc::channel();
    let abort_flag = Arc::new(AtomicBool::new(false));
    app.chat_session.event_rx = Some(rx);
    app.chat_session.assistant_buffer.clear();
    app.chat_session.pending_tools.clear();
    app.chat_session.in_flight = true;
    app.chat_session.abort_flag = Some(abort_flag.clone());

    thread::spawn(move || {
        // Private current-thread tokio runtime — we only run one task.
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(ChatUpdate::Error(ChatErrorKind::Network(format!(
                    "tokio runtime: {e}"
                ))));
                return;
            }
        };
        runtime.block_on(run_request(endpoint, request, bearer, tx, abort_flag));
    });

    Ok(())
}

/// The async request body that runs on the background thread. Forwards
/// parsed events through `tx`. Polls `abort_flag` between each stream
/// event — when the main thread sets it, the loop breaks and the
/// stream is dropped, emitting a `Done` so the drain path finalizes
/// whatever partial text was received.
async fn run_request(
    endpoint: String,
    request: ChatRequest,
    bearer: Option<String>,
    tx: Sender<ChatUpdate>,
    abort_flag: Arc<AtomicBool>,
) {
    let client = Client::new(endpoint);
    let mut stream = match client.stream(&request, bearer.as_deref()).await {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(ChatUpdate::Error(chat_error_kind(e)));
            return;
        }
    };

    let mut pending_tool: Option<(String, String, String)> = None; // (id, name, json)

    while let Some(event) = stream.next().await {
        if abort_flag.load(Ordering::Relaxed) {
            let _ = tx.send(ChatUpdate::Done);
            return;
        }
        match event {
            Ok(ChatEvent::Text(t)) => {
                if tx.send(ChatUpdate::Text(t)).is_err() {
                    return;
                }
            }
            Ok(ChatEvent::ToolStart { id, name }) => {
                pending_tool = Some((id, name, String::new()));
            }
            Ok(ChatEvent::ToolDelta(json)) => {
                if let Some(t) = pending_tool.as_mut() {
                    t.2.push_str(&json);
                }
            }
            Ok(ChatEvent::BlockStop) => {
                if let Some((id, name, json)) = pending_tool.take() {
                    let args: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
                    if tx.send(ChatUpdate::ToolCall { id, name, args }).is_err() {
                        return;
                    }
                }
            }
            Ok(ChatEvent::Done) => {
                let _ = tx.send(ChatUpdate::Done);
                return;
            }
            Err(e) => {
                let _ = tx.send(ChatUpdate::Error(chat_error_kind(e)));
                return;
            }
        }
    }
    // Stream ended without an explicit Done — treat it as clean close.
    let _ = tx.send(ChatUpdate::Done);
}

/// Map a typed `ChatError` into a `ChatErrorKind` we can ship over the
/// channel. Stringifies network errors (they carry a reqwest::Error which
/// isn't straightforward to preserve as a rich value).
fn chat_error_kind(err: ChatError) -> ChatErrorKind {
    match err {
        ChatError::RateLimited { body } => ChatErrorKind::RateLimited { body },
        ChatError::Http { status, body } => ChatErrorKind::Http { status, body },
        ChatError::Network(e) => ChatErrorKind::Network(e.to_string()),
        ChatError::Parse(msg) => ChatErrorKind::Parse(msg),
    }
}

/// Produce a (log line, chat line) pair for a [`ChatErrorKind`]. The log
/// line is one-line terse so the status bar ticker stays readable; the
/// chat line is multi-paragraph with actionable next steps.
pub fn friendly_error_text(kind: &ChatErrorKind) -> (String, String) {
    match kind {
        ChatErrorKind::RateLimited { .. } => (
            "chat rate limit reached".to_string(),
            "You've hit the anonymous chat quota (3 messages/day). \
             Run `vcad login` in another terminal to sign in and keep chatting."
                .to_string(),
        ),
        ChatErrorKind::Http { status: 404, .. } => (
            "chat endpoint 404".to_string(),
            "The chat endpoint isn't reachable. If you're running a local \
             api, set VCAD_CHAT_ENDPOINT (e.g. http://localhost:3001/api/chat)."
                .to_string(),
        ),
        ChatErrorKind::Http { status: 401, .. } | ChatErrorKind::Http { status: 403, .. } => (
            "chat auth rejected".to_string(),
            "Your stored token was rejected. Run `vcad login --token <jwt>` \
             to replace it or `vcad logout` to go anonymous."
                .to_string(),
        ),
        ChatErrorKind::Http { status, .. } => (
            format!("chat HTTP {status}"),
            format!(
                "Chat endpoint returned {status}. Set VCAD_CHAT_ENDPOINT if \
                 you're targeting a non-default URL."
            ),
        ),
        ChatErrorKind::Network(e) => (
            format!("chat network: {e}"),
            "Couldn't reach the chat endpoint — network down or the URL is \
             wrong. Check your connection or set VCAD_CHAT_ENDPOINT."
                .to_string(),
        ),
        ChatErrorKind::Parse(e) => (
            format!("chat parse: {e}"),
            format!("Chat stream parse failed: {e}. This is likely a bug."),
        ),
    }
}

/// Drain any ready events from the background thread and apply them to
/// `App`. Non-blocking — safe to call every frame from the main loop.
///
/// Two-phase: first pull every ready update into a local `Vec` so we can
/// drop the `rx` borrow, then mutate `app` freely while applying them.
pub fn drain_chat_events(app: &mut App) {
    let mut updates: Vec<ChatUpdate> = Vec::new();
    let mut stream_closed = false;
    {
        let Some(rx) = app.chat_session.event_rx.as_ref() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(update) => updates.push(update),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    stream_closed = true;
                    break;
                }
            }
        }
    }

    for update in updates {
        match update {
            ChatUpdate::Text(t) => apply_text(app, &t),
            ChatUpdate::ToolCall { id, name, args } => apply_tool_call(app, id, &name, args),
            ChatUpdate::Done => {
                finalize_turn_and_maybe_chain(app);
                return;
            }
            ChatUpdate::Error(kind) => {
                let (log_line, chat_line) = friendly_error_text(&kind);
                app.log(LogLevel::Error, "chat", log_line);
                app.chat.assistant(chat_line);
                app.chat_session.in_flight = false;
                app.chat_session.event_rx = None;
                app.chat_session.pending_tools.clear();
                app.chat_session.assistant_buffer.clear();
                app.chat_session.abort_flag = None;
                return;
            }
        }
    }

    if stream_closed {
        finalize_turn_and_maybe_chain(app);
    }
}

/// Append streaming text to the assistant response. Rewrites the last
/// chat line in place so the render loop shows token-by-token streaming.
fn apply_text(app: &mut App, delta: &str) {
    app.chat_session.assistant_buffer.push_str(delta);
    let buf = app.chat_session.assistant_buffer.clone();
    // Replace or append the last assistant line.
    let last_is_assistant = app
        .chat
        .lines
        .last()
        .is_some_and(|l| l.kind == ChatLineKind::Assistant);
    if last_is_assistant {
        if let Some(last) = app.chat.lines.last_mut() {
            last.text = buf;
        }
    } else {
        app.chat.assistant(buf);
    }
}

/// Execute a tool call against the live document and record it for the
/// next follow-up request. Shows a ✓/✗ chip in the chat panel.
fn apply_tool_call(app: &mut App, id: String, name: &str, args: Value) {
    let result = execute_crud(name, &args, &mut app.document);
    let is_error = matches!(result.status, vcad_chat::ExecutionStatus::Error);
    let status_icon = if is_error { "\u{2717}" } else { "\u{2713}" };
    app.chat.debug(format!(
        "{status_icon} {name} — {}",
        result.result.lines().next().unwrap_or("")
    ));

    app.chat_session.pending_tools.push(ToolUseRecord {
        id,
        name: name.to_string(),
        input: args,
        result: result.result,
        is_error,
    });

    // Re-evaluate meshes since the document changed.
    if let Err(e) = app.evaluate() {
        app.log(LogLevel::Error, "eval", format!("evaluate after tool: {e}"));
    }
}

/// Commit the assistant's current turn to the history and, if any tools
/// were executed, fire a follow-up request with tool_result blocks so
/// the model can continue.
fn finalize_turn_and_maybe_chain(app: &mut App) {
    let text = std::mem::take(&mut app.chat_session.assistant_buffer);
    let tools = std::mem::take(&mut app.chat_session.pending_tools);

    // Build the assistant message. If there were tool calls, use a
    // content-block array that mirrors Anthropic's native shape; otherwise
    // a plain text message is enough.
    if !tools.is_empty() {
        let mut assistant_blocks: Vec<Value> = Vec::new();
        if !text.is_empty() {
            assistant_blocks.push(json!({ "type": "text", "text": text }));
        }
        for t in &tools {
            assistant_blocks.push(json!({
                "type": "tool_use",
                "id": t.id,
                "name": t.name,
                "input": t.input,
            }));
        }
        let assistant_msg = ChatMessage {
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(assistant_blocks),
        };
        append_history(&assistant_msg);
        app.chat_session.messages.push(assistant_msg);

        // Corresponding user message carries the tool_result blocks.
        let result_blocks: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mut obj = json!({
                    "type": "tool_result",
                    "tool_use_id": t.id,
                    "content": t.result,
                });
                if t.is_error {
                    if let Some(map) = obj.as_object_mut() {
                        map.insert("is_error".into(), Value::Bool(true));
                    }
                }
                obj
            })
            .collect();
        let tool_result_msg = ChatMessage {
            role: MessageRole::User,
            content: MessageContent::Blocks(result_blocks),
        };
        append_history(&tool_result_msg);
        app.chat_session.messages.push(tool_result_msg);

        // Close out the current stream and fire the follow-up.
        app.chat_session.event_rx = None;
        app.chat_session.in_flight = false;
        if let Err(e) = start_chat_turn(app) {
            app.log(LogLevel::Error, "chat", e.to_string());
        }
        return;
    }

    // No tool calls this turn — commit plain text and end.
    if !text.is_empty() {
        let msg = ChatMessage {
            role: MessageRole::Assistant,
            content: MessageContent::Text(text),
        };
        append_history(&msg);
        app.chat_session.messages.push(msg);
    }
    app.chat_session.in_flight = false;
    app.chat_session.event_rx = None;
    app.chat_session.abort_flag = None;
}

/// Push a new user message onto the conversation history. Called from the
/// chat panel's Enter handler before [`start_chat_turn`].
pub fn push_user_message(app: &mut App, text: String) {
    let msg = ChatMessage {
        role: MessageRole::User,
        content: MessageContent::Text(text),
    };
    append_history(&msg);
    app.chat_session.messages.push(msg);
}

// ---------------------------------------------------------------------------
// History persistence
// ---------------------------------------------------------------------------

/// Path to the chat history file: piggybacks on vcad_chat::token_path's
/// parent directory so we don't have to introduce a new dep on
/// `directories` here.
fn history_path() -> Option<PathBuf> {
    let token = vcad_chat::token_path().ok()?;
    token.parent().map(|p| p.join("chat.jsonl"))
}

/// Append a single finalized message to `chat.jsonl`. Best-effort — a
/// persistence failure logs via `eprintln!` (captured by log_capture in
/// the TUI) but never blocks the chat flow.
fn append_history(msg: &ChatMessage) {
    let Some(path) = history_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string(msg) else {
        return;
    };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{json}");
    }
}

/// Load the persisted chat history as a `Vec<ChatMessage>`. Missing or
/// unreadable files produce an empty vec. Individual unparseable lines
/// are skipped so a corrupted tail doesn't lose the whole file.
pub fn load_history() -> Vec<ChatMessage> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    let Ok(body) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<ChatMessage>(line).ok())
        .collect()
}

/// Render a loaded message into the chat panel's display lines so the
/// history is visible on launch. Messages with content-block arrays
/// (tool_use / tool_result) are expanded into debug lines so the user
/// sees what happened at each turn, not just raw JSON.
pub fn rehydrate_display(app: &mut App, messages: &[ChatMessage]) {
    use crate::ui::chat::ChatLineKind;
    for msg in messages {
        match &msg.content {
            MessageContent::Text(text) => {
                let kind = match msg.role {
                    MessageRole::User => ChatLineKind::User,
                    MessageRole::Assistant => ChatLineKind::Assistant,
                };
                app.chat.lines.push(crate::ui::chat::ChatLine {
                    text: text.clone(),
                    kind,
                });
            }
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    let Some(obj) = block.as_object() else {
                        continue;
                    };
                    let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match ty {
                        "text" => {
                            let text = obj
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let kind = match msg.role {
                                MessageRole::User => ChatLineKind::User,
                                MessageRole::Assistant => ChatLineKind::Assistant,
                            };
                            app.chat.lines.push(crate::ui::chat::ChatLine {
                                text,
                                kind,
                            });
                        }
                        "tool_use" => {
                            let name =
                                obj.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                            app.chat.debug(format!("\u{2726} {name}"));
                        }
                        "tool_result" => {
                            let is_error = obj
                                .get("is_error")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let summary = obj
                                .get("content")
                                .and_then(|v| v.as_str())
                                .unwrap_or("(result)");
                            let icon = if is_error { "\u{2717}" } else { "\u{2713}" };
                            app.chat.debug(format!("{icon} {summary}"));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TUI → prompt adapters. `parts_from_document` lives in vcad-chat now so
// both the TUI and the web (via vcad-kernel-wasm bindings) compute parts
// identically. `selection_from_app` stays here because it reaches into
// App::selected + App::document in a TUI-specific way.
// ---------------------------------------------------------------------------

fn selection_from_app(app: &App) -> Vec<SelectionInfo> {
    app.selected
        .iter()
        .filter_map(|nid| {
            let node = app.document.nodes.get(nid)?;
            let op_value = serde_json::to_value(&node.op).ok()?;
            let geometry_type = op_value
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some(SelectionInfo {
                part_id: nid.to_string(),
                part_name: node
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("part {nid}")),
                geometry_type,
            })
        })
        .collect()
}

fn selection_to_selected_parts(selection: &[SelectionInfo]) -> Vec<SelectedPart> {
    selection
        .iter()
        .map(|s| SelectedPart {
            part_id: s.part_id.clone(),
            part_name: s.part_name.clone(),
            geometry_type: s.geometry_type.clone(),
        })
        .collect()
}
