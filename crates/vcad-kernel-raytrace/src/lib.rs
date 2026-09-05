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
//! - [`bvh`] - Per-solid bounding volume hierarchy (the BLAS)
//! - [`tlas`] - Top-level hierarchy over placed instances, so a scene costs
//!   O(log parts) per ray instead of O(parts) and repeated parts share one BLAS
//!
//! # Example
//!
//! ```ignore
//! use vcad_kernel_math::{Point3, Vec3};
//! use vcad_kernel_primitives::make_cube;
//! // `build_brep` lives on the extension trait: the generic hierarchy has
//! // an inherent `build` that takes the geometry ready-made.
//! use vcad_kernel_raytrace::{BrepBvh, Bvh, Ray};
//!
//! let brep = make_cube(10.0, 10.0, 10.0);
//! let bvh = Bvh::build_brep(&brep);
//!
//! let ray = Ray::new(
//!     Point3::new(-5.0, 5.0, 5.0),
//!     Vec3::new(1.0, 0.0, 0.0),
//! );
//!
//! let hits = bvh.trace(&ray);
//! ```

pub mod bvh;
pub mod cpu;
pub mod intersect;
pub mod pathtrace;
mod ray;
pub mod tlas;
pub mod trim;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use bvh::{BrepBvh, BrepGeom, Bvh};
pub use cpu::{render_scene, render_scene_samples, CpuRenderer};
pub use pathtrace::{
    render_into, studio_rig, AreaLight, Camera, Environment, Film, Ground, Object,
    PathTraceOptions, Pbr, Scene,
};
pub use ray::{Ray, RayHit};
pub use tlas::{BrepTlas, Instance, InstanceHit, Tlas};
