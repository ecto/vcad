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

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;

use vcad_chat::{
    anthropic_tools, build_system_prompt, execute_crud, load_token, stream::ChatRequest,
    ChatContext, ChatError, ChatEvent, ChatMessage, Client, MessageContent, MessageRole,
    PartInfo, SelectedPart, SelectionInfo,
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
        /// Unique id from the server — used by M4c when we send back a
        /// `tool_result` content block to chain the next turn.
        #[allow(dead_code)]
        id: String,
        name: String,
        args: Value,
    },
    /// Stream finished cleanly.
    Done,
    /// Fatal stream error. The caller shows this in the chat panel.
    Error(String),
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
    /// True while a request is in flight; blocks new sends and the
    /// assistant row renders with a spinner.
    pub in_flight: bool,
}

impl ChatSession {
    /// True when a request is streaming or tool-execution is pending.
    pub fn is_busy(&self) -> bool {
        self.in_flight
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
    let parts = parts_from_document(app);
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
    app.chat_session.event_rx = Some(rx);
    app.chat_session.assistant_buffer.clear();
    app.chat_session.in_flight = true;

    thread::spawn(move || {
        // Private current-thread tokio runtime — we only run one task.
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(ChatUpdate::Error(format!("tokio runtime: {e}")));
                return;
            }
        };
        runtime.block_on(run_request(endpoint, request, bearer, tx));
    });

    Ok(())
}

/// The async request body that runs on the background thread. Forwards
/// parsed events through `tx`.
async fn run_request(
    endpoint: String,
    request: ChatRequest,
    bearer: Option<String>,
    tx: Sender<ChatUpdate>,
) {
    let client = Client::new(endpoint);
    let mut stream = match client.stream(&request, bearer.as_deref()).await {
        Ok(s) => s,
        Err(ChatError::RateLimited { body }) => {
            let _ = tx.send(ChatUpdate::Error(format!("rate limited: {body}")));
            return;
        }
        Err(e) => {
            let _ = tx.send(ChatUpdate::Error(e.to_string()));
            return;
        }
    };

    let mut pending_tool: Option<(String, String, String)> = None; // (id, name, json)

    while let Some(event) = stream.next().await {
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
                let _ = tx.send(ChatUpdate::Error(e.to_string()));
                return;
            }
        }
    }
    // Stream ended without an explicit Done — treat it as clean close.
    let _ = tx.send(ChatUpdate::Done);
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
            ChatUpdate::ToolCall { id: _, name, args } => apply_tool_call(app, &name, args),
            ChatUpdate::Done => {
                finalize_assistant(app);
                app.chat_session.in_flight = false;
                app.chat_session.event_rx = None;
                return;
            }
            ChatUpdate::Error(e) => {
                app.log(LogLevel::Error, "chat", e.clone());
                app.chat.assistant(format!("[error] {e}"));
                app.chat_session.in_flight = false;
                app.chat_session.event_rx = None;
                return;
            }
        }
    }

    if stream_closed {
        app.chat_session.in_flight = false;
        app.chat_session.event_rx = None;
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

/// Execute a tool call against the live document and show a chip line.
fn apply_tool_call(app: &mut App, name: &str, args: Value) {
    // Snapshot the assistant buffer as a committed line so the tool chip
    // appears underneath any partial text already streamed.
    finalize_assistant(app);

    let result = execute_crud(name, &args, &mut app.document);
    let status_icon = match result.status {
        vcad_chat::ExecutionStatus::Success => "\u{2713}",
        vcad_chat::ExecutionStatus::Error => "\u{2717}",
    };
    app.chat.debug(format!(
        "{status_icon} {name} — {}",
        result.result.lines().next().unwrap_or("")
    ));

    // Re-evaluate meshes since the document changed.
    if let Err(e) = app.evaluate() {
        app.log(LogLevel::Error, "eval", format!("evaluate after tool: {e}"));
    }
}

/// Commit the current streaming assistant buffer into `messages` as a
/// finalized assistant message and reset the buffer.
fn finalize_assistant(app: &mut App) {
    let buf = std::mem::take(&mut app.chat_session.assistant_buffer);
    if !buf.is_empty() {
        app.chat_session.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            content: MessageContent::Text(buf),
        });
    }
}

/// Push a new user message onto the conversation history. Called from the
/// chat panel's Enter handler before [`start_chat_turn`].
pub fn push_user_message(app: &mut App, text: String) {
    app.chat_session.messages.push(ChatMessage {
        role: MessageRole::User,
        content: MessageContent::Text(text),
    });
}

// ---------------------------------------------------------------------------
// Document → prompt adapters
// ---------------------------------------------------------------------------

fn parts_from_document(app: &App) -> Vec<PartInfo> {
    app.document
        .roots
        .iter()
        .filter_map(|entry| {
            let node = app.document.nodes.get(&entry.root)?;
            let op_value = serde_json::to_value(&node.op).ok()?;
            let kind = op_value
                .get("type")
                .and_then(|t| t.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| "csg_op".to_string());
            let params = op_value
                .as_object()
                .map(|o| {
                    let mut clone = o.clone();
                    clone.remove("type");
                    Value::Object(clone)
                })
                .unwrap_or(Value::Null);
            Some(PartInfo {
                id: entry.root.to_string(),
                name: node.name.clone().unwrap_or_else(|| format!("part {}", entry.root)),
                kind: kind.clone(),
                nodes: vec![vcad_chat::NodeInfo {
                    node_id: entry.root.to_string(),
                    node_type: kind,
                    params,
                }],
            })
        })
        .collect()
}

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
