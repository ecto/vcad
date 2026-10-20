//! Cross-host parity fixtures (`tests/parity/*.json`).
//!
//! A fixture is a command log plus the outcomes and final
//! [`DocumentApi::state_hash`](crate::DocumentApi::state_hash) every host
//! must reproduce. This module holds the shared format; each host drives
//! its own binding (native, C ABI, WASM) and compares against `expect`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One replayable command log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// Replica id the engine is created with (makes stable ids deterministic).
    pub replica: u64,
    /// Steps, applied in order.
    pub steps: Vec<Step>,
    /// Golden results.
    pub expect: Expect,
}

/// A single step: a raw command or an AI tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    /// `{"command": DocCommand}`
    Command {
        /// The command JSON.
        command: Value,
    },
    /// `{"tool": "create", "args": {…}}`
    Tool {
        /// Tool name.
        tool: String,
        /// Tool arguments.
        args: Value,
    },
}

/// Golden results for a fixture.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Expect {
    /// Per-step outcome, see [`outcome`].
    pub outcomes: Vec<String>,
    /// Final state hash.
    pub hash: String,
}

/// Summarize a result envelope as `ok:<createdFeatureId|->` or `err:<kind>`.
pub fn outcome(envelope_json: &str) -> String {
    let v: Value = serde_json::from_str(envelope_json).unwrap_or(Value::Null);
    if v["ok"] == Value::Bool(true) {
        format!(
            "ok:{}",
            v["result"]["createdFeatureId"].as_str().unwrap_or("-")
        )
    } else {
        format!("err:{}", v["error"]["kind"].as_str().unwrap_or("?"))
    }
}

/// Replay a fixture through a native [`DocumentApi`](crate::DocumentApi).
pub fn replay_native(fixture: &Fixture) -> Expect {
    let mut api = crate::DocumentApi::new(vcad_crdt::ReplicaId(fixture.replica));
    let outcomes = fixture
        .steps
        .iter()
        .map(|step| {
            outcome(&match step {
                Step::Command { command } => api.apply_json_envelope(&command.to_string()),
                Step::Tool { tool, args } => api.apply_tool_envelope(tool, &args.to_string()),
            })
        })
        .collect();
    Expect {
        outcomes,
        hash: api.state_hash(),
    }
}
