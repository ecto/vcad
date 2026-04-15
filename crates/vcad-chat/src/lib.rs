//! vcad-chat — AI chat contract for vcad.
//!
//! Owns the client half of the vcad chat protocol so the TUI (and later
//! native / web Rust frontends) can hit `https://vcad.io/api/chat` and
//! execute the returned tool calls without touching TypeScript.
//!
//! Modules:
//! - [`schemas`]  — tool schemas sourced from [`vcad_ir::CsgOp`]
//! - [`tools`]    — Anthropic tool definitions, mirrors `toAnthropicTools`
//! - [`prompt`]   — system prompt builder, mirrors `buildSystemPrompt`
//! - [`stream`]   — HTTP client + NDJSON stream parser
//! - [`executor`] — `execute_crud`, port of `executeCrud`
//! - [`auth`]     — token storage and refresh
//! - [`state`]    — [`ChatSession`] and [`Message`] types
//!
//! The TS reference lives in `packages/core/src/commands/registry.ts`,
//! `packages/app/src/lib/chat-api.ts`, and `api/chat.ts`.

// Pure-Rust surface: always compiled, wasm-compatible.
pub mod executor;
pub mod prompt;
pub mod schemas;
pub mod tools;

// Native-only surface: HTTP client, async stream parser, token storage.
// Gated behind the default-enabled `native` feature; WASM consumers
// (`vcad-kernel-wasm`, eventual Dioxus/Blitz web target) depend on
// vcad-chat with `default-features = false` and skip these modules.
#[cfg(feature = "native")]
pub mod auth;
#[cfg(feature = "native")]
pub mod stream;

#[cfg(feature = "native")]
pub use auth::{
    clear_token, generate_device_code, load_token, open_browser, poll_for_token, save_token,
    token_path, AuthError, DeviceCode, Token,
};
pub use executor::{execute_crud, ExecutionResult, ExecutionStatus};
pub use prompt::{
    build_system_prompt, parts_from_document, type_catalog, NodeInfo, PartInfo, SelectionInfo,
};
pub use schemas::{all_schemas, type_enum};
#[cfg(feature = "native")]
pub use stream::{
    ChatContext, ChatError, ChatEvent, ChatMessage, ChatRequest, Client, MessageContent,
    MessageRole, SelectedPart,
};
pub use tools::{anthropic_tools, AnthropicTool};
