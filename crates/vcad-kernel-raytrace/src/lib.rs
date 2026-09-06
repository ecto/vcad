#![warn(missing_docs)]

//! The BRep adapter for `kosm-render`.
//!
//! The renderer itself — rays, acceleration structures, the path tracer, and
//! (behind `--features gpu`) the whole wgpu compute pipeline: the BSDF, the
//! environment, the accumulator, the device-side history and the denoiser —
//! lives in `kosm-render`, generic over geometry. This crate supplies the BRep
//! half of that seam and re-exports the rest, so a caller still sees one API.
//!
//! What is genuinely here is the B-rep knowledge: intersecting analytic
//! surfaces (planes, cylinders, spheres, cones, tori, bilinear and B-spline
//! patches) without tessellation, and deciding whether a hit's *(u, v)* falls
//! inside a trimmed face's boundary loops. That is what buys pixel-perfect
//! silhouettes at any zoom level, and it is not a lighting fact.
//!
//! On the CPU that means implementing `kosm_render::Geometry` over BRep faces
//! ([`bvh::BrepGeom`]); on the GPU it means packing those faces into the five
//! storage buffers kosm-render leaves for geometry and supplying `brep.wgsl`,
//! which defines the five functions its integrator calls. See
//! [`gpu::BrepGeometry`] and `kosm_render::gpu::geometry` for the contract.
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
//! ```
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
/// Environments the renderer builds for itself: the synthesised studio
/// HDRIs and the Radiance `.hdr` reader. Re-exported from `kosm-render` —
/// none of it knew what a B-rep was.
pub use kosm_render::env;

pub mod intersect;
pub mod pathtrace;
mod ray;
pub mod tlas;
pub mod trim;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use bvh::{BrepBvh, BrepGeom, Bvh, FlatPrims, FlatTriangle};
pub use cpu::{render_scene, render_scene_samples, CpuRenderer};
pub use pathtrace::{
    render_into, studio_rig, AreaLight, Camera, Environment, Film, Ground, Object,
    PathTraceOptions, Pbr, Scene, Sun,
};
pub use ray::{Ray, RayHit};
pub use tlas::{BrepTlas, Instance, InstanceHit, Tlas};
