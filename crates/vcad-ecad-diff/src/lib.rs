#![warn(missing_docs)]
//! Differentiable PCB design — solve trace geometry by gradient.
//!
//! Mirrors [`vcad_kernel_constraints::symbolic::CompiledSystem`], but the
//! residuals are *design objectives* (impedance targets) built from the
//! `vcad_ecad_sim::impedance` `Scalar`-generic leaves rather than sketch
//! constraints. It traces the residuals symbolically via `tang-expr`,
//! sparsity-prunes, differentiates the non-zero Jacobian entries, compiles to
//! closures, and solves with the generic Levenberg-Marquardt driver
//! ([`vcad_kernel_constraints::levenberg_marquardt`]) — with box bounds applied
//! through the driver's projection hook.
//!
//! This is the Rust counterpart of the TypeScript `size_impedance` MCP tool:
//! same model, gradients instead of finite differences, and a path that scales
//! to coupled multi-parameter problems where the symbolic sparse Jacobian pays.
pub mod design;
pub mod pdn;

pub use design::{DesignSystem, ResidualFn};
pub use pdn::{PdnEdge, PdnSystem};
