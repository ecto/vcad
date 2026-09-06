//! Ray representation and intersection results.
//!
//! Both moved to `kosm-render`: a ray does not know what it is about to hit,
//! and neither does a hit know what it landed on beyond the primitive index
//! its geometry can look up. What used to be `RayHit::face_id` is now
//! [`BrepGeom::face_id`](crate::bvh::BrepGeom::face_id) applied to
//! [`Hit::prim`](kosm_render::Hit::prim).

/// A ray in 3D space defined by origin and direction.
pub use kosm_render::Ray;

/// Result of a ray-surface intersection.
pub use kosm_render::Hit as RayHit;
