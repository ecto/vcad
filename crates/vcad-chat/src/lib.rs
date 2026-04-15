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

pub mod schemas;
pub mod tools;

pub use schemas::{all_schemas, type_enum};
pub use tools::{anthropic_tools, AnthropicTool};
