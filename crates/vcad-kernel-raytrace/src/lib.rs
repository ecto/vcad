#![warn(missing_docs)]

//! Direct BRep ray tracing for the vcad kernel.
//!
//! This crate provides ray tracing capabilities that work directly with BRep
//! surfaces (planes, cylinders, spheres, cones, tori, etc.) without tessellation,
//! achieving pixel-perfect silhouettes at any zoom level.
//!
//! # Architecture
//!
//! - [`Ray`] - Ray representation with origin and direction
//! - [`RayHit`] - Intersection result with surface parameters
//! - [`intersect`] - Ray-surface intersection algorithms for each surface type
//! - [`trim`] - Point-in-face testing for trimmed surfaces
//! - [`bvh`] - Bounding volume hierarchy for acceleration
//!
//! # Example
//!
//! ```ignore
//! use vcad_kernel_raytrace::{Ray, Bvh};
//! use vcad_kernel_primitives::make_cube;
//!
//! let brep = make_cube(10.0, 10.0, 10.0);
//! let bvh = Bvh::build(&brep);
//!
//! let ray = Ray::new(
//!     Point3::new(-5.0, 5.0, 5.0),
//!     Vec3::new(1.0, 0.0, 0.0),
//! );
//!
//! let hits = bvh.trace(&ray);
//! ```

mod ray;
pub mod intersect;
pub mod trim;
pub mod bvh;
pub mod cpu;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use ray::{Ray, RayHit};
pub use bvh::Bvh;
pub use cpu::{CpuRenderer, render_scene};
