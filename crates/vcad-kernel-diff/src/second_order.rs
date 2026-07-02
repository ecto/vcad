//! M10 — exact directional second derivatives of the volume QoI.
//!
//! The seam already prices `dx/dθ` (velocities) and, through them,
//! `dV/dθ = Σ_i (∂V/∂x_i)·ẋ_i`. The honest second derivative of the volume
//! along one parameter is
//!
//! ```text
//! d²V/dθ² = Σ_ij (∂²V/∂x_i∂x_j) ẋ_i ẋ_j  +  Σ_i (∂V/∂x_i) ẍ_i,
//! ```
//!
//! two contributions this module carries in full:
//!
//! - the **curvature of `V` in the node positions**, contracted with the
//!   velocities twice — nonzero even when every node moves at constant speed
//!   (the boolean hole's `V(r)` is quadratic in `r` with *zero* node
//!   acceleration, yet `d²V/dr² ≠ 0`); and
//! - the **node accelerations** `ẍ_i = d²x_i/dθ²`.
//!
//! Both fall out of one pass of the shared generic volume integral
//! [`crate::mesh_volume`] over the nested scalar `Dual<Dual<f64>>`: seed each
//! node coordinate as `((x, ẋ), (ẋ, ẍ))` and read `V`, `dV/dθ`, `d²V/dθ²`
//! off the value / first-tangent / second-tangent slots. `Dual<Dual<f64>>`
//! satisfies `tang::Scalar` (every `Dual<S: Scalar>` does), so `mesh_volume`
//! compiles at that type unchanged — the genericity that was always the
//! second-order on-ramp.
//!
//! # Node accelerations
//!
//! - **Lift nodes** (`SurfaceUv`): the surface point `x(θ) = S(u, v;
//!   fields(θ))` is evaluated through the nested-dual lift
//!   ([`crate::lift_surface_second`]) with every seeded field packed as
//!   `((f, ḟ), (ḟ, f̈))`, so `ẍ = ∂x/∂field·field̈ + ∂²x/∂field²·fielḋ ²`
//!   comes out exactly for **every** surface kind — the linear kinds
//!   (plane/cylinder/sphere, where the second term vanishes) and the
//!   nonlinear ones (cone's `tan α`, torus radii) alike.
//! - **Vertex / Boundary nodes** solve the second-order implicit system
//!   ([`crate::constraint_row_2`]): the same row gradients as the velocity
//!   solve, the frozen tangential completion reused verbatim, only the
//!   right-hand side carries the curvature — including the cone/torus
//!   non-constant `∇²g` terms. Tangency-completion rows are linear in `x`
//!   and the surface center, so their second-order form is the first-order
//!   [`crate::tangency_rows`] fed the acceleration seeds.
//!
//! What this computes, exactly: the volume second derivative with the node
//! accelerations of plane/cylinder/sphere nodes carried in full. It is not a
//! generic Hessian over many parameters (that is [`crate::gauss_newton_hvp`]'s
//! job for least-squares QoIs) — it is `d²V/dθ²` along one seeded direction,
//! with every term stated.

use std::collections::{BTreeSet, HashMap};

use tang::Dual;
use vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{refine_boundary_point, FrozenError, FrozenPlan, NodeRecipe};

use crate::seam::{
    assemble_vertex_rows, checked_index, incidence_context, vertex_incident_surfaces,
};
use crate::{
    constraint_row, constraint_row_2, lift_surface_second, mesh_volume, solve_vertex_velocity,
    tangency_rows, ConstraintRow, DiffError, DualSurface2, ParamSeeding,
};

/// A parameter's first- **and** second-order seeding: how θ moves each
/// surface's fields to first order (`velocity`) and to second order
/// (`acceleration`).
///
/// For the common case where θ enters the fields *linearly* (a radius equal
/// to θ, a face translating at constant rate), the `acceleration` seeding is
/// empty and every node acceleration is zero — yet `d²V/dθ²` is still
/// generally nonzero (it is the position-curvature term). A genuinely
/// nonlinear θ → field map (a radius `r(θ) = θ²`, a cone half-angle) fills in
/// the `acceleration` seeds with the field's `d²/dθ²`.
#[derive(Debug, Clone, Default)]
pub struct SecondOrderSeeding {
    /// First-order seeding (`d field/dθ`) — identical to the forward seam's
    /// [`ParamSeeding`], so a first-order problem lifts to second order by
    /// adding accelerations and nothing else.
    pub velocity: ParamSeeding,
    /// Second-order seeding (`d² field/dθ²`), reusing the same
    /// [`crate::SurfaceSeed`] vocabulary with each rate/velocity read as the
    /// field's acceleration. Empty for fields linear in θ.
    pub acceleration: ParamSeeding,
}

impl SecondOrderSeeding {
    /// A seeding whose field velocities are `velocity` and whose field
    /// accelerations are all zero (the θ-linear case).
    pub fn linear(velocity: ParamSeeding) -> Self {
        Self {
            velocity,
            acceleration: ParamSeeding::new(),
        }
    }
}

/// A frozen mesh with first- and second-order sensitivities: node `i` of
/// `positions` has velocity `velocities[i]` (`dx/dθ`) and acceleration
/// `accelerations[i]` (`d²x/dθ²`).
#[derive(Debug, Clone)]
pub struct SeamMeshSecond {
    /// Node positions `x(θ)`.
    pub positions: Vec<Point3>,
    /// Node velocities `dx/dθ`.
    pub velocities: Vec<Vec3>,
    /// Node accelerations `d²x/dθ²`.
    pub accelerations: Vec<Vec3>,
    /// Frozen connectivity (copied from the plan).
    pub triangles: Vec<[u32; 3]>,
    /// Recipes (copied from the plan) so callers can select node classes.
    pub recipes: Vec<NodeRecipe>,
}

/// Second-order rows of a topology vertex: the same incident-surface and
/// tangency-pair walk as the first-order [`assemble_vertex_rows`], but each
/// row's rhs carries the acceleration curvature. Gradients are identical to
/// first order, so [`solve_vertex_velocity`] recovers `ẍ`.
fn assemble_vertex_rows_second(
    brep: &BRepSolid,
    incident: &BTreeSet<usize>,
    seeding: &SecondOrderSeeding,
    x: &Point3,
    xdot: &Vec3,
) -> Result<Vec<ConstraintRow>, DiffError> {
    let mut rows = Vec::new();
    for &sidx in incident {
        let surface = brep.geometry.surfaces[sidx].as_ref();
        rows.push(constraint_row_2(
            surface,
            seeding.velocity.get(sidx),
            seeding.acceleration.get(sidx),
            x,
            xdot,
        )?);
    }
    for &pidx in incident {
        let plane = brep.geometry.surfaces[pidx].as_ref();
        let Some(p) = plane.as_any().downcast_ref::<vcad_kernel_geom::Plane>() else {
            continue;
        };
        let n = *p.normal_dir.as_ref();
        for &sidx in incident {
            if sidx == pidx {
                continue;
            }
            let surface = brep.geometry.surfaces[sidx].as_ref();
            // A tangency constraint q·(x − center) = 0 is linear in x and the
            // center (q constant), so its second total derivative is
            // q·ẍ = q·center̈ — the first-order row fed the acceleration seeds.
            for row in tangency_rows(n, surface, seeding.acceleration.get(sidx), x)? {
                rows.push(row);
            }
        }
    }
    Ok(rows)
}

/// Evaluate a frozen plan against its capture-time B-rep with **first- and
/// second-order** analytic sensitivities.
///
/// `brep` must be the capture-time B-rep, exactly as for
/// [`crate::evaluate_with_sensitivity`] (the same signature and anchor checks
/// hold). The first-order path is identical to that function; the addition is
/// the per-node acceleration `ẍ` (see the module docs for the kinematics).
/// Every surface kind with an implicit form is supported — plane, cylinder,
/// sphere, cone, and torus.
pub fn evaluate_with_second_derivative(
    brep: &BRepSolid,
    plan: &FrozenPlan,
    seeding: &SecondOrderSeeding,
) -> Result<SeamMeshSecond, DiffError> {
    let ci = checked_index(brep, plan)?;
    let topo = &brep.topology;
    let ctx = incidence_context(brep, &ci.faces);
    let vel = &seeding.velocity;
    let acc = &seeding.acceleration;

    // Lift each referenced face surface once to the nested-dual scalar with
    // both seedings applied — shared by every sample on the face.
    let mut lifted: HashMap<u32, DualSurface2> = HashMap::new();

    let anchor_check = |node: usize, p: &Point3, anchor: &Point3| -> Result<(), DiffError> {
        let d2 = (*p - *anchor).norm_squared();
        if d2 > crate::seam::ANCHOR_TOL * crate::seam::ANCHOR_TOL {
            return Err(FrozenError::CorrespondenceLost {
                node,
                distance: d2.sqrt(),
            }
            .into());
        }
        Ok(())
    };

    let mut positions = Vec::with_capacity(plan.nodes.len());
    let mut velocities = Vec::with_capacity(plan.nodes.len());
    let mut accelerations = Vec::with_capacity(plan.nodes.len());

    for (i, recipe) in plan.nodes.iter().enumerate() {
        match *recipe {
            NodeRecipe::TopoVertex { vertex } => {
                let vid = *ci
                    .vertices
                    .get(vertex as usize)
                    .ok_or(FrozenError::RecipeOutOfRange)?;
                let x = topo.vertices[vid].point;
                anchor_check(i, &x, &plan.base_positions[i])?;
                let incident = vertex_incident_surfaces(brep, &ctx, vid, &x);
                let rows1: Vec<ConstraintRow> = assemble_vertex_rows(brep, &incident, vel, &x)?
                    .into_iter()
                    .map(|(_, row)| row)
                    .collect();
                let xdot = solve_vertex_velocity(&rows1)?;
                let rows2 = assemble_vertex_rows_second(brep, &incident, seeding, &x, &xdot)?;
                let xddot = solve_vertex_velocity(&rows2)?;
                positions.push(x);
                velocities.push(xdot);
                accelerations.push(xddot);
            }
            NodeRecipe::SurfaceUv { face, u, v } => {
                let surface_index = {
                    let face_id = *ci
                        .faces
                        .get(face as usize)
                        .ok_or(FrozenError::RecipeOutOfRange)?;
                    topo.faces[face_id].surface_index
                };
                let uv = Point2::new(u, v);
                if let std::collections::hash_map::Entry::Vacant(e) = lifted.entry(face) {
                    let surface = brep.geometry.surfaces[surface_index].as_ref();
                    e.insert(lift_surface_second(
                        surface,
                        vel.get(surface_index),
                        acc.get(surface_index),
                    )?);
                }
                let (p, vx, ax) = lifted[&face].evaluate_with_acceleration(uv);
                anchor_check(i, &p, &plan.base_positions[i])?;
                positions.push(p);
                velocities.push(vx);
                accelerations.push(ax);
            }
            NodeRecipe::Boundary { face_a, face_b, .. } => {
                let sidx = |slot: u32| -> Result<usize, DiffError> {
                    let face_id = *ci
                        .faces
                        .get(slot as usize)
                        .ok_or(FrozenError::RecipeOutOfRange)?;
                    Ok(topo.faces[face_id].surface_index)
                };
                let (sidx_a, sidx_b) = (sidx(face_a)?, sidx(face_b)?);
                let sa = brep.geometry.surfaces[sidx_a].as_ref();
                let sb = brep.geometry.surfaces[sidx_b].as_ref();
                let anchor = plan.base_positions[i];
                let x = refine_boundary_point(sa, sb, &anchor)
                    .ok_or(FrozenError::BoundarySolveFailed { node: i })?;
                anchor_check(i, &x, &anchor)?;
                let rows1 = vec![
                    constraint_row(sa, vel.get(sidx_a), &x)?,
                    constraint_row(sb, vel.get(sidx_b), &x)?,
                ];
                let xdot = solve_vertex_velocity(&rows1)?;
                let rows2 = vec![
                    constraint_row_2(sa, vel.get(sidx_a), acc.get(sidx_a), &x, &xdot)?,
                    constraint_row_2(sb, vel.get(sidx_b), acc.get(sidx_b), &x, &xdot)?,
                ];
                let xddot = solve_vertex_velocity(&rows2)?;
                positions.push(x);
                velocities.push(xdot);
                accelerations.push(xddot);
            }
        }
    }

    Ok(SeamMeshSecond {
        positions,
        velocities,
        accelerations,
        triangles: plan.triangles.clone(),
        recipes: plan.nodes.clone(),
    })
}

/// Volume, `dV/dθ`, and `d²V/dθ²` of a second-order seam mesh, in one pass of
/// the shared generic integral over `Dual<Dual<f64>>`.
///
/// Each node coordinate is packed as `((x, ẋ), (ẋ, ẍ))` — the standard
/// nested-dual seeding of a scalar function of one variable — so the returned
/// triple is `(V, dV/dθ, d²V/dθ²)` read off the value, first-tangent, and
/// second-tangent slots. The result is **exact** for the frozen mesh: both
/// the `∂²V/∂x²:ẋẋ` position-curvature term and the `∂V/∂x·ẍ`
/// node-acceleration term are carried (see the module docs).
pub fn volume_with_second_derivative(seam: &SeamMeshSecond) -> (f64, f64, f64) {
    type Dd = Dual<Dual<f64>>;
    let pack = |x: f64, v: f64, a: f64| -> Dd { Dual::new(Dual::new(x, v), Dual::new(v, a)) };
    let pts: Vec<tang::Point3<Dd>> = seam
        .positions
        .iter()
        .zip(&seam.velocities)
        .zip(&seam.accelerations)
        .map(|((p, v), a)| {
            tang::Point3::new(
                pack(p.x, v.x, a.x),
                pack(p.y, v.y, a.y),
                pack(p.z, v.z, a.z),
            )
        })
        .collect();
    let vol = mesh_volume(&pts, &seam.triangles);
    (vol.real.real, vol.real.dual, vol.dual.dual)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Dual<Dual<f64>>` satisfies `tang::Scalar`, and the nested-dual seeding
    /// recovers `f`, `f'`, `f''` of a scalar polynomial — the on-ramp probe.
    #[test]
    fn nested_dual_recovers_second_derivative() {
        type Dd = Dual<Dual<f64>>;
        // f(θ) = θ³ at θ = 2, with θ seeded as the variable.
        let theta = 2.0;
        let seed: Dd = Dual::new(Dual::new(theta, 1.0), Dual::new(1.0, 0.0));
        let f = seed * seed * seed;
        assert!((f.real.real - 8.0).abs() < 1e-12); // 2³
        assert!((f.real.dual - 12.0).abs() < 1e-12); // 3·2²
        assert!((f.dual.dual - 12.0).abs() < 1e-12); // 6·2
    }
}
