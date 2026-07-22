#![warn(missing_docs)]

//! MechEval grader.
//!
//! Loads a task JSON (see `mecheval/tasks/SCHEMA.md`), dispatches each check
//! against a candidate `.vcad` file, and produces a run blob (see
//! `mecheval/runs/SCHEMA.md`).
//!
//! # Status
//!
//! v0.0 skeleton. The check-type vocabulary is fully enumerated and JSON
//! parsing works; individual check implementations are stubs returning
//! [`CheckOutcome::NotImplemented`]. Wiring each check to the relevant
//! `vcad-kernel-*` crate is a follow-up step.

pub mod anti_cheese;
pub mod blob;
pub mod check;
pub mod dfm;
pub mod eval;
pub mod fillets;
pub mod fit;
pub mod fit_physics;
pub mod grader;
pub mod holes;
pub mod pcb;
pub mod suite_c;
pub mod task;
pub mod visual;

pub use blob::{CheckOutcome, CheckRecord, RunBlob, Summary};
pub use check::CheckSpec;
pub use grader::{grade, GraderError};
pub use task::{AntiCheese, Limits, Suite, Task};
