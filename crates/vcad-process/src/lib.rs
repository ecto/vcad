//! Planar semiconductor process emulation — the digital twin of a fab,
//! v0.
//!
//! Takes a GDSII layout (masks) plus a [`Recipe`] (deposit, etch, grow,
//! implant, planarize) and produces geometry as [`vcad_ir::Document`]s:
//!
//! - [`simulate_3d`] — the 3D film stack over a (windowed) die region.
//! - [`cross_section`] — the classic textbook process cross-section along
//!   a cut line, with real gaps where material was etched away.
//!
//! See the crate README for the process model and its (deliberate)
//! planar-v0 approximations.

#![warn(missing_docs)]

pub mod error;
pub mod masks;
pub mod recipe;
pub mod section;
pub mod sim;

mod bridge;

pub use bridge::{cross_section, simulate_3d};
pub use error::{ProcessError, Result};
pub use recipe::{Axis, CutLine, Polarity, ProcessStep, Recipe, SI_CONSUMED_PER_OXIDE};
pub use sim::{simulate, Film, FilmKind, Masks};
