//! Live end-to-end test of the built-in agent loop against frog api, with
//! REAL tool execution on the Rust engine (the Swift `FROG_LIVE` test can't
//! link off-Mac; this runs anywhere cargo does).
//!
//! ```text
//! FROG_BASE_URL=https://api.frog.toys/v1 FROG_API_KEY=… \
//!   cargo run -p vcad-app --features agent-live --example agent_live
//! ```
//!
//! Env: `FROG_BASE_URL`, `FROG_API_KEY` (required, never printed),
//! `FROG_MODEL` (default: first id from `GET /models`), `FROG_WAIT_SECS`
//! (status poll timeout, default 600), `FROG_POLL_SECS` (default 15).
//!
//! Mirrors `FrogClient.swift` / `FrogAgent.swift`: OpenAI chat completions
//! with tools, key-sorted JSON, `prompt_cache_key` derived from a hash of the
//! system+tools prefix, `user-agent: vcad/<version>`. Prints one JSON report
//! line on stdout. Exit 0 = pass, 1 = fail, 2 = endpoint never served.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use vcad_app::{agent, DocumentApi};
use vcad_crdt::ReplicaId;

const PROMPT: &str = "Create a 20mm cube, then move it up 10mm in Z, then inspect it.";
const MAX_ROUNDS: usize = 12;
const READ_TOOLS: [&str; 2] = ["describe_scene", "inspect"];

fn env_secs(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn emit(report: &Value, code: i32) -> ! {
    println!("{report}");
    std::process::exit(code);
}

fn main() {
    let started = chrono_now();
    let base = std::env::var("FROG_BASE_URL").unwrap_or_default();
    let key = std::env::var("FROG_API_KEY").unwrap_or_default();
    if base.is_empty() || key.is_empty() {
        emit(
            &json!({"pass": false, "status": "config_error", "started": started,
                    "error": "FROG_BASE_URL and FROG_API_KEY must be set"}),
            1,
        );
    }
    let base = base.trim_end_matches('/').to_string();
    let user_agent = format!("vcad/{}", env!("CARGO_PKG_VERSION"));
    let http = reqwest::blocking::Client::builder()
        .user_agent(user_agent.clone())
        .timeout(Duration::from_secs(300))
        .build()
        .expect("http client");

    // 1. Wait for `serving: true`.
    let status_url = format!("{}/status?format=json", base.trim_end_matches("/v1"));
    let wait = Duration::from_secs(env_secs("FROG_WAIT_SECS", 600));
    let poll = Duration::from_secs(env_secs("FROG_POLL_SECS", 15).max(1));
    let t0 = Instant::now();
    let mut polls = 0u32;
    let mut last_status = Value::Null;
    loop {
        polls += 1;
        let serving = match http.get(&status_url).send().and_then(|r| r.json::<Value>()) {
            Ok(v) => {
                let s = v.get("serving").and_then(Value::as_bool) == Some(true);
                last_status = v;
                s
            }
            Err(e) => {
                last_status = json!({"poll_error": e.to_string()});
                false
            }
        };
        if serving {
            break;
        }
        if t0.elapsed() >= wait {
            emit(
                &json!({"pass": false, "status": "not_serving", "started": started,
                        "polls": polls, "waited_s": t0.elapsed().as_secs(),
                        "last_status": last_status}),
                2,
            );
        }
        eprintln!("[agent_live] not serving yet (poll {polls}), retrying");
        std::thread::sleep(poll);
    }
    let waited_s = t0.elapsed().as_secs_f64();

    // 2. Model id.
    let model = match std::env::var("FROG_MODEL").ok().filter(|s| !s.is_empty()) {
        Some(m) => m,
        None => http
            .get(format!("{base}/models"))
            .bearer_auth(&key)
            .send()
            .and_then(|r| r.json::<Value>())
            .ok()
            .and_then(|v| v["data"][0]["id"].as_str().map(str::to_string))
            .unwrap_or_else(|| {
                emit(
                    &json!({"pass": false, "status": "no_model", "started": started}),
                    1,
                )
            }),
    };

    // 3. Manifest prefix + cache key (serde_json Values are key-sorted).
    let manifest = agent::manifest();
    let system = manifest["system"].as_str().unwrap_or_default().to_string();
    let tools = manifest["tools"].clone();
    let prefix = json!({"system": {"role": "system", "content": system}, "tools": tools});
    let digest = Sha256::digest(serde_json::to_vec(&prefix).expect("prefix"));
    let cache_key = format!(
        "{}-{}",
        manifest["cache_key"].as_str().unwrap_or("vcad-agent"),
        digest[..8].iter().map(|b| format!("{b:02x}")).collect::<String>()
    );

    // 4. Tool loop on a fresh document.
    let mut doc = DocumentApi::new(ReplicaId(1));
    let mut messages = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": PROMPT}),
    ];
    let mut turns = Vec::new();
    let mut final_text = String::new();
    let mut error: Option<String> = None;
    for round in 0..MAX_ROUNDS {
        let body = json!({
            "model": model, "messages": messages, "stream": false,
            "prompt_cache_key": cache_key, "tools": tools, "tool_choice": "auto",
        });
        let t = Instant::now();
        let resp = http
            .post(format!("{base}/chat/completions"))
            .bearer_auth(&key)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).expect("body"))
            .send();
        let latency_ms = t.elapsed().as_millis() as u64;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                error = Some(format!("round {round}: transport: {e}"));
                break;
            }
        };
        let code = resp.status().as_u16();
        let v: Value = resp.json().unwrap_or(Value::Null);
        if !(200..300).contains(&code) {
            error = Some(format!("round {round}: HTTP {code}: {}", v.to_string().chars().take(500).collect::<String>()));
            break;
        }
        let usage = &v["usage"];
        let msg = v["choices"][0]["message"].clone();
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let mut call_log = Vec::new();
        messages.push(msg.clone());
        for call in &calls {
            let id = call["id"].as_str().unwrap_or_default();
            let name = call["function"]["name"].as_str().unwrap_or_default();
            let args = call["function"]["arguments"].as_str().unwrap_or("{}");
            let args = if args.trim().is_empty() { "{}" } else { args };
            let content = if READ_TOOLS.contains(&name) {
                agent::read_tool_envelope(&doc, name, args)
            } else {
                doc.apply_tool_envelope(name, args)
            };
            let ok = serde_json::from_str::<Value>(&content)
                .map(|e| e["ok"] == json!(true))
                .unwrap_or(false);
            call_log.push(json!({"name": name, "arguments": args, "ok": ok,
                                 "result": content.chars().take(400).collect::<String>()}));
            messages.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
        }
        turns.push(json!({
            "round": round, "latency_ms": latency_ms,
            "prompt_tokens": usage["prompt_tokens"],
            "completion_tokens": usage["completion_tokens"],
            "cached_tokens": usage["prompt_tokens_details"]["cached_tokens"],
            "finish_reason": v["choices"][0]["finish_reason"],
            "tool_calls": call_log,
        }));
        if calls.is_empty() {
            final_text = msg["content"].as_str().unwrap_or_default().to_string();
            break;
        }
        if round + 1 == MAX_ROUNDS {
            error = Some("too many tool rounds".into());
        }
    }

    // 5. Assert from the engine document.
    let state = doc.get_state();
    let ids: Vec<String> = state.parts.iter().map(|p| p.id().to_string()).collect();
    let bbox = ids.first().and_then(|id| doc.part_world_bbox(id));
    let mut checks = Vec::new();
    if ids.len() != 1 {
        checks.push(format!("expected exactly 1 part, got {}", ids.len()));
    }
    match bbox {
        Some((lo, hi)) => {
            if (lo[2] - 10.0).abs() > 1e-6 || (hi[2] - 30.0).abs() > 1e-6 {
                checks.push(format!("bbox z {}..{}, expected 10..30", lo[2], hi[2]));
            }
        }
        None => checks.push("no bbox for the part".into()),
    }
    let pass = error.is_none() && checks.is_empty();
    emit(
        &json!({
            "pass": pass, "status": if pass { "pass" } else { "fail" },
            "started": started, "model": model, "cache_key": cache_key,
            "user_agent": user_agent, "waited_s": waited_s, "polls": polls,
            "prompt": PROMPT, "turns": turns, "final_text": final_text,
            "parts": ids, "bbox": bbox.map(|(lo, hi)| json!({"min": lo, "max": hi})),
            "failures": checks, "error": error,
        }),
        if pass { 0 } else { 1 },
    );
}

/// Unix seconds (no chrono dependency).
fn chrono_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
