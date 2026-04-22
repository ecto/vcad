//! Local AI bridge.
//!
//! Proxies the chat loop through a locally-running inference server
//! (currently just Ollama on `127.0.0.1:11434`). Doing it in Rust means
//! the webview's CSP stays tight (no extra `connect-src` hole), and lets
//! us later extend to server management / other engines without touching
//! the frontend.

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

const OLLAMA_BASE: &str = "http://127.0.0.1:11434";

#[derive(Serialize)]
pub struct LocalAiProbe {
    pub ollama_url: Option<String>,
    pub models: Vec<String>,
}

#[derive(Deserialize)]
struct OllamaTagsResp {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

#[tauri::command]
pub async fn local_ai_probe() -> LocalAiProbe {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return LocalAiProbe {
                ollama_url: None,
                models: vec![],
            };
        }
    };

    let resp = match client.get(format!("{OLLAMA_BASE}/api/tags")).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => {
            return LocalAiProbe {
                ollama_url: None,
                models: vec![],
            };
        }
    };

    let tags: OllamaTagsResp = match resp.json().await {
        Ok(t) => t,
        Err(_) => {
            return LocalAiProbe {
                ollama_url: Some(OLLAMA_BASE.to_string()),
                models: vec![],
            };
        }
    };

    LocalAiProbe {
        ollama_url: Some(OLLAMA_BASE.to_string()),
        models: tags.models.into_iter().map(|m| m.name).collect(),
    }
}

#[derive(Deserialize)]
pub struct LocalAiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind")]
pub enum LocalAiEvent {
    Delta { text: String },
    Done,
    Error { message: String },
}

#[derive(Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: Option<OllamaChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct OllamaChunkMessage {
    #[serde(default)]
    content: String,
}

/// Stream a chat completion from Ollama to the frontend.
///
/// The frontend passes a `Channel<LocalAiEvent>`; we forward each text
/// delta as `{ kind: "delta", text }` and emit `{ kind: "done" }` at the
/// end, or `{ kind: "error", message }` on failure.
#[tauri::command]
pub async fn local_ai_chat_stream(
    model: String,
    messages: Vec<LocalAiMessage>,
    on_event: Channel<LocalAiEvent>,
) -> Result<(), String> {
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": model,
        "stream": true,
        "messages": messages
            .into_iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect::<Vec<_>>(),
    });

    let mut resp = match client
        .post(format!("{OLLAMA_BASE}/api/chat"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let _ = on_event.send(LocalAiEvent::Error {
                message: e.to_string(),
            });
            return Err(e.to_string());
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let msg = resp
            .text()
            .await
            .unwrap_or_else(|_| format!("HTTP {status}"));
        let _ = on_event.send(LocalAiEvent::Error {
            message: msg.clone(),
        });
        return Err(msg);
    }

    // Ollama streams NDJSON. Each chunk may contain one or more JSON lines;
    // buffer partial lines across chunks.
    let mut buffer = String::new();
    loop {
        match resp.chunk().await {
            Ok(Some(bytes)) => {
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(nl) = buffer.find('\n') {
                    let line = buffer.drain(..=nl).collect::<String>();
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<OllamaChunk>(line) {
                        Ok(chunk) => {
                            if let Some(err) = chunk.error {
                                let _ = on_event.send(LocalAiEvent::Error {
                                    message: err.clone(),
                                });
                                return Err(err);
                            }
                            if let Some(msg) = chunk.message {
                                if !msg.content.is_empty() {
                                    let _ =
                                        on_event.send(LocalAiEvent::Delta { text: msg.content });
                                }
                            }
                            if chunk.done {
                                let _ = on_event.send(LocalAiEvent::Done);
                                return Ok(());
                            }
                        }
                        Err(_) => {
                            // Malformed line — skip, Ollama sometimes emits
                            // partial fragments at the stream tail.
                        }
                    }
                }
            }
            Ok(None) => {
                let _ = on_event.send(LocalAiEvent::Done);
                return Ok(());
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = on_event.send(LocalAiEvent::Error {
                    message: msg.clone(),
                });
                return Err(msg);
            }
        }
    }
}
