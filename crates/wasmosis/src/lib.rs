//! # wasmosis
//!
//! Lazy WASM module splitting for Rust.
//!
//! Split large WASM binaries into lazy-loadable modules. Functions are
//! automatically assigned to modules based on feature gates and dependencies.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use wasm_bindgen::prelude::*;
//!
//! // Module inferred from feature gate → "physics"
//! #[cfg(feature = "physics")]
//! #[wasm_bindgen]
//! pub fn create_physics_env(doc: &str) -> PhysicsSim {
//!     vcad_kernel_physics::RobotEnv::new(doc)
//! }
//!
//! // Module inferred from dependency → "gpu"
//! #[wasm_bindgen]
//! pub fn init_gpu() -> bool {
//!     vcad_kernel_gpu::GpuContext::init()
//! }
//!
//! // No inference trigger → "core" (always loaded)
//! #[wasm_bindgen]
//! pub fn create_cube(x: f64, y: f64, z: f64) -> Solid {
//!     // ...
//! }
//! ```
//!
//! ## Explicit Override
//!
//! Use `#[module("name")]` when you need to override automatic inference:
//!
//! ```rust,ignore
//! use wasmosis::module;
//!
//! #[module("advanced")]
//! #[wasm_bindgen]
//! pub fn experimental_fillet(solid: &Solid) -> Solid {
//!     // ...
//! }
//! ```
//!
//! ## How Inference Works
//!
//! Functions are assigned to modules in this priority:
//!
//! 1. **Explicit**: `#[module("name")]` annotation
//! 2. **Feature gate**: `#[cfg(feature = "physics")]` → `physics` module
//! 3. **Dependency**: `vcad_kernel_physics::` in body → `physics` module
//! 4. **Default**: `core` module (always loaded)
//!
//! ## CLI
//!
//! ```bash
//! # Analyze with inference reasoning
//! wasmosis analyze src/lib.rs --show-inference
//!
//! # Generate separate crates
//! wasmosis codegen src/lib.rs -o ./generated -n my-kernel
//!
//! # Build with wasm-pack
//! wasmosis build -c ./generated -o ./dist
//! ```

// Re-export the proc-macro
pub use wasmosis_macro::module;

/// The name of the custom section used by wasmosis.
pub const SECTION_NAME: &str = "wasmosis_module";
