//! HTTP client + NDJSON stream parser for `POST /api/chat`.
//!
//! The endpoint streams newline-delimited `data: {...}` lines with five
//! event types — `text`, `tool_start`, `tool_delta`, `block_stop`, `done`.
//! This module parses those into [`ChatEvent`] values. The wire shape is
//! defined by `api/chat.ts:142-201` (server) and `packages/app/src/lib/chat-api.ts:109-143`
//! (client); both are authoritative and must stay in sync with this parser.

use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::tools::AnthropicTool;

/// A single event parsed out of the `/api/chat` NDJSON stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    /// An assistant text delta. The server emits one of these per
    /// `content_block_delta` from Anthropic.
    Text(String),
    /// Start of a tool-use content block — the tool has a stable id and
    /// a canonical name.
    ToolStart { id: String, name: String },
    /// Partial JSON for the *current* tool's `input` argument. These
    /// concatenate across many events and parse as JSON only at
    /// `BlockStop`.
    ToolDelta(String),
    /// End of the current content block. When the block was a tool, this
    /// is the signal to concatenate pending `ToolDelta`s and parse them.
    BlockStop,
    /// End of the full response.
    Done,
}

/// Structured chat errors.
#[derive(Debug, Error)]
pub enum ChatError {
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("rate limited: {body}")]
    RateLimited { body: String },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("stream parse error: {0}")]
    Parse(String),
}

/// Role for a chat message — mirrors Anthropic's `role` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

/// A single chat message. `content` is either a plain string or an array of
/// content blocks (for tool results, images, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: MessageContent,
}

/// Polymorphic content field — matches how `chat-api.ts:11-14` and the
/// Anthropic Messages API model content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<Value>),
}

/// Selected-geometry context sent alongside the messages so the server can
/// fold it into the system prompt (the TUI builds its own prompt today, but
/// we still send the structured context for parity with the web).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatContext {
    pub selected_parts: Vec<SelectedPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedPart {
    pub part_id: String,
    pub part_name: String,
    pub geometry_type: String,
}

/// Full request body for `POST /api/chat`. Field names match `chat-api.ts:55-65`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub context: ChatContext,
    pub tools: Vec<AnthropicTool>,
    #[serde(rename = "systemPrompt")]
    pub system_prompt: String,
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

/// Thin wrapper around a `reqwest::Client` that knows the vcad chat endpoint
/// and produces a parsed [`ChatEvent`] stream.
#[derive(Debug, Clone)]
pub struct Client {
    endpoint: String,
    http: reqwest::Client,
}

impl Client {
    /// Build a client pointed at an absolute chat endpoint (e.g.
    /// `https://vcad.io/api/chat`).
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Open a chat stream. `bearer` is the Supabase JWT from `auth::load_token`;
    /// pass `None` to hit the anonymous rate limit.
    pub async fn stream(
        &self,
        request: &ChatRequest,
        bearer: Option<&str>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent, ChatError>> + Send>>, ChatError> {
        let mut req = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(request);
        if let Some(token) = bearer {
            req = req.header("Authorization", format!("Bearer {token}"));
        }

        let response = req.send().await?;
        let status = response.status();

        if status.as_u16() == 429 {
            let body = response.text().await.unwrap_or_default();
            return Err(ChatError::RateLimited { body });
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ChatError::Http {
                status: status.as_u16(),
                body,
            });
        }

        // Adapt the byte stream into a Stream<Item = Result<ChatEvent, _>>.
        let byte_stream = response.bytes_stream();
        let parsed = NdjsonStream::new(byte_stream);
        Ok(Box::pin(parsed))
    }
}

// ---------------------------------------------------------------------------
// Stream adapter — accumulates bytes into lines, parses each `data: …`
// ---------------------------------------------------------------------------

struct NdjsonStream<S> {
    inner: S,
    buffer: String,
    pending: std::collections::VecDeque<ChatEvent>,
    done: bool,
}

impl<S> NdjsonStream<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: String::new(),
            pending: std::collections::VecDeque::new(),
            done: false,
        }
    }
}

impl<S> Stream for NdjsonStream<S>
where
    S: Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    type Item = Result<ChatEvent, ChatError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        // Reborrow once and take disjoint field borrows below — the naive
        // `self.inner.poll_next_unpin(cx)` followed by `&mut self.buffer`
        // trips E0499 otherwise.
        let this = &mut *self;

        loop {
            if let Some(ev) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(ev)));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match this.inner.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = match std::str::from_utf8(&chunk) {
                        Ok(s) => s.to_string(),
                        Err(e) => {
                            return Poll::Ready(Some(Err(ChatError::Parse(format!(
                                "non-utf8 chunk: {e}"
                            )))))
                        }
                    };
                    if let Err(e) = parse_chunk_into(&text, &mut this.buffer, &mut this.pending) {
                        return Poll::Ready(Some(Err(e)));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ChatError::Network(e))))
                }
                Poll::Ready(None) => {
                    this.done = true;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Pure parse helper — feed raw chunk bytes (as UTF-8) into a rolling buffer
/// and emit any events whose lines are now complete. Exposed for testing.
pub fn parse_chunk_into(
    chunk: &str,
    buffer: &mut String,
    out: &mut std::collections::VecDeque<ChatEvent>,
) -> Result<(), ChatError> {
    buffer.push_str(chunk);
    // Split on `\n`, keeping the final (possibly incomplete) line in the buffer.
    while let Some(pos) = buffer.find('\n') {
        let mut line = buffer[..pos].to_string();
        // Drop the consumed line (including the `\n`).
        buffer.drain(..=pos);
        // Trim trailing `\r` from CRLF endings.
        if line.ends_with('\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        if let Some(payload) = line.strip_prefix("data: ") {
            let ev = match parse_event(payload) {
                Ok(Some(ev)) => ev,
                Ok(None) => continue,
                Err(e) => return Err(e),
            };
            out.push_back(ev);
        }
    }
    Ok(())
}

/// Parse a single `data:` payload body into a [`ChatEvent`]. Returns `None`
/// for events we recognize but don't surface (none today).
fn parse_event(payload: &str) -> Result<Option<ChatEvent>, ChatError> {
    let value: Value = serde_json::from_str(payload)
        .map_err(|e| ChatError::Parse(format!("bad json `{payload}`: {e}")))?;
    let ty = value
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ChatError::Parse(format!("event missing type: {payload}")))?;

    let ev = match ty {
        "text" => {
            let text = value
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            ChatEvent::Text(text)
        }
        "tool_start" => {
            let id = value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ChatEvent::ToolStart { id, name }
        }
        "tool_delta" => {
            let json = value
                .get("json")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ChatEvent::ToolDelta(json)
        }
        "block_stop" => ChatEvent::BlockStop,
        "done" => ChatEvent::Done,
        other => {
            return Err(ChatError::Parse(format!("unknown event type: {other}")));
        }
    };
    Ok(Some(ev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn parse_one(chunk: &str) -> Vec<ChatEvent> {
        let mut buf = String::new();
        let mut out = VecDeque::new();
        parse_chunk_into(chunk, &mut buf, &mut out).unwrap();
        out.into_iter().collect()
    }

    #[test]
    fn parses_text_event() {
        let events = parse_one("data: {\"type\":\"text\",\"text\":\"hello\"}\n");
        assert_eq!(events, vec![ChatEvent::Text("hello".into())]);
    }

    #[test]
    fn parses_tool_round_trip() {
        let raw = concat!(
            "data: {\"type\":\"tool_start\",\"id\":\"t1\",\"name\":\"create\"}\n",
            "data: {\"type\":\"tool_delta\",\"json\":\"{\\\"type\\\":\"}\n",
            "data: {\"type\":\"tool_delta\",\"json\":\"\\\"cube\\\"}\"}\n",
            "data: {\"type\":\"block_stop\"}\n",
            "data: {\"type\":\"done\"}\n",
        );
        let events = parse_one(raw);
        assert_eq!(
            events,
            vec![
                ChatEvent::ToolStart {
                    id: "t1".into(),
                    name: "create".into(),
                },
                ChatEvent::ToolDelta("{\"type\":".into()),
                ChatEvent::ToolDelta("\"cube\"}".into()),
                ChatEvent::BlockStop,
                ChatEvent::Done,
            ]
        );
    }

    #[test]
    fn handles_chunk_boundaries_mid_line() {
        let mut buf = String::new();
        let mut out = VecDeque::new();
        // First chunk ends mid-line.
        parse_chunk_into("data: {\"type\":\"text\",\"t", &mut buf, &mut out).unwrap();
        assert!(out.is_empty(), "no full line yet");
        // Second chunk completes the first line and starts another.
        parse_chunk_into(
            "ext\":\"hello\"}\ndata: {\"type\":\"done\"}\n",
            &mut buf,
            &mut out,
        )
        .unwrap();
        let events: Vec<_> = out.into_iter().collect();
        assert_eq!(
            events,
            vec![ChatEvent::Text("hello".into()), ChatEvent::Done]
        );
    }

    #[test]
    fn crlf_line_endings_ok() {
        let events = parse_one("data: {\"type\":\"text\",\"text\":\"ok\"}\r\n");
        assert_eq!(events, vec![ChatEvent::Text("ok".into())]);
    }

    #[test]
    fn ignores_blank_lines_and_non_data_lines() {
        let events = parse_one("\n\nheartbeat\ndata: {\"type\":\"done\"}\n");
        assert_eq!(events, vec![ChatEvent::Done]);
    }

    #[test]
    fn unknown_event_is_an_error() {
        let mut buf = String::new();
        let mut out = VecDeque::new();
        let err =
            parse_chunk_into("data: {\"type\":\"mystery\"}\n", &mut buf, &mut out).unwrap_err();
        assert!(matches!(err, ChatError::Parse(_)));
    }

    #[test]
    fn request_serializes_with_snake_and_camel_keys() {
        use crate::tools::anthropic_tools;
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: MessageContent::Text("hi".into()),
            }],
            context: ChatContext::default(),
            tools: anthropic_tools(),
            system_prompt: "you are vcad".into(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("messages").is_some());
        assert!(json.get("context").is_some());
        assert!(json.get("tools").is_some());
        // Matches the web client's key name.
        assert!(json.get("systemPrompt").is_some());
    }
}
