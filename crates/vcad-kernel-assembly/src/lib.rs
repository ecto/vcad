#![warn(missing_docs)]
//! Posed assemblies: transforms as the single source of truth, plus the
//! checks that make a wrong transform impossible to ship quietly.
//!
//! An assembly document is a set of named part definitions, a set of
//! **instances** (a part reference plus a transform, optionally an
//! exploded-view offset), and a set of **mates** — declarative assertions
//! about what those transforms are supposed to achieve. Evaluating the
//! document poses every instance into world space; everything downstream
//! reads the poses from here rather than re-deriving them.
//!
//! ```ignore
//! use vcad_kernel_assembly::{check_interference, check_mates, pose_document, InterferenceOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let doc = vcad_loon::eval_vcad(&std::fs::read_to_string("stack.loon")?, None)?;
//! let posed = pose_document(&doc)?;
//!
//! for check in check_mates(&posed, &doc.mates)? {
//!     println!("{}", check.summary());
//! }
//!
//! // Respect the model's intentional 0.05 mm seam overlaps.
//! let report = check_interference(&posed, &InterferenceOptions::with_tolerance(0.05));
//! println!("{}", report.summary());
//! # Ok(())
//! # }
//! ```
//!
//! ## What is checked
//!
//! - [`MateKind::Coaxial`](vcad_ir::MateKind::Coaxial) — two parts' reference
//!   axes lie on one line (antiparallel counts: a flipped part is coaxial).
//! - [`MateKind::PlanarOffset`](vcad_ir::MateKind::PlanarOffset) — two parts
//!   sit a stated distance apart along an axis. The z-stack written down once.
//! - [`MateKind::PatternPhase`](vcad_ir::MateKind::PatternPhase) — two parts
//!   carrying `n`-fold circular patterns are phase-aligned under whatever
//!   flip and clocking the poses apply.
//! - [`check_interference`] — mesh-mesh overlap over the posed assembly, with
//!   an explicit tolerance for deliberate modelling overlaps.
//!
//! ## Why `pattern-phase` exists
//!
//! A dual-rotor axial-flux motor was specified as "front rotor: flip, clock
//! 60°". The rotors carry 10-pole magnet arrays, so the pole pitch is 36° and
//! 60 mod 36 = 24 — a 12° pole misalignment (60° electrical), which shipped
//! and was caught only later by redoing the arithmetic by hand for the next
//! revision. Clocking 180° instead gives 180 mod 36 = 0: exact alignment.
//!
//! That arithmetic is invisible in prose and trivially wrong by hand, and it
//! is one modular reduction for a checker that can see both poses. It is the
//! reason this crate is a checker and not a solver: nothing was
//! under-constrained, the number was simply wrong, and no amount of solving
//! would have said so.
//!
//! ## Not in scope
//!
//! **Kinematic degrees of freedom.** A mate never moves a part and knows
//! nothing about joints. Articulation stays with [`vcad_ir::Joint`] and
//! [`vcad_eval::solve_forward_kinematics`], whose results this crate consumes
//! when a document has a joint graph. "Backdrive the train and check it still
//! clears" needs a DOF model on top of these checks; it is deliberately left
//! for follow-up work.

pub mod interference;
pub mod mate_check;
pub mod pose;

pub use interference::{check_interference, Interference, InterferenceOptions, InterferenceReport};
pub use mate_check::{check_mate, check_mates, MateCheck, MateError};
pub use pose::{mesh_bounds, pose_document, Affine, PoseError, PosedAssembly, PosedPart};
