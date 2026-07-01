//! Assembly of the seam: per-node positions and velocities for a frozen
//! plan, plus dual-valued quantities of interest built on top of them.

use std::collections::{BTreeSet, HashMap};

use tang::{Dual, Scalar};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{
    canonical_index, signature_with_index, FrozenError, FrozenPlan, NodeRecipe,
};
use vcad_kernel_topo::VertexId;

use crate::{
    constraint_row, lift_surface, solve_vertex_velocity, surface_residual, DiffError, ParamSeeding,
};

/// Incidence tolerance (mm) for treating a topology vertex as lying on a
/// surface. Boolean/trim vertices land on their defining surfaces to
/// machine precision; this is far above that and far below feature size.
const INCIDENCE_TOL: f64 = 1e-6;

/// A frozen mesh with first-order sensitivities: node `i` of `positions`
/// corresponds to node `i` of `velocities` (dx/dθ).
#[derive(Debug, Clone)]
pub struct SeamMesh {
    /// Node positions x(θ).
    pub positions: Vec<Point3>,
    /// Node velocities dx/dθ.
    pub velocities: Vec<Vec3>,
    /// Frozen connectivity (copied from the plan).
    pub triangles: Vec<[u32; 3]>,
    /// Recipes (copied from the plan) so callers can select node classes
    /// (e.g. rim vertices vs interior surface samples) in their gates.
    pub recipes: Vec<NodeRecipe>,
}

/// Evaluate a frozen plan against a B-rep with analytic sensitivities.
///
/// `brep` must be the **capture-time B-rep** (the primal model at the base
/// θ): recipes resolve entities by capture-time traversal index, which is
/// only meaningful on the B-rep the plan was captured from. Perturbed
/// rebuilds are the FD oracle's business (`frozen::evaluate_plan`, which
/// matches entities geometrically instead).
///
/// - `SurfaceUv` nodes go through the lift-bridge: the face's surface is
///   lifted to `Dual<f64>` with its seed and evaluated at the frozen
///   `(u, v)` (Pillar 2 — exact forward AD).
/// - `TopoVertex` nodes are differentiated implicitly through the system of
///   their adjacent surfaces (Pillar 3).
///
/// The plan's topology signature is enforced first; a mismatch is a hard
/// error ([`FrozenError::TopologyChanged`]).
pub fn evaluate_with_sensitivity(
    brep: &BRepSolid,
    plan: &FrozenPlan,
    seeding: &ParamSeeding,
) -> Result<SeamMesh, DiffError> {
    let ci = canonical_index(brep);
    let actual = signature_with_index(brep, &ci);
    if actual != plan.signature {
        return Err(FrozenError::TopologyChanged {
            expected: plan.signature,
            actual,
        }
        .into());
    }

    let topo = &brep.topology;

    // Adjacent surface indices per topology vertex (distinct indices only).
    let mut adjacent: HashMap<VertexId, BTreeSet<usize>> = HashMap::new();
    for &face_id in &ci.faces {
        let face = &topo.faces[face_id];
        let loops = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
        for loop_id in loops {
            for he in topo.loop_half_edges(loop_id) {
                adjacent
                    .entry(topo.half_edges[he].origin)
                    .or_default()
                    .insert(face.surface_index);
            }
        }
    }

    // Lift each referenced face surface once (they are shared by many nodes).
    let mut lifted: HashMap<u32, crate::DualSurface> = HashMap::new();

    let mut positions = Vec::with_capacity(plan.nodes.len());
    let mut velocities = Vec::with_capacity(plan.nodes.len());
    for recipe in &plan.nodes {
        match *recipe {
            NodeRecipe::TopoVertex { vertex } => {
                let vid = *ci
                    .vertices
                    .get(vertex as usize)
                    .ok_or(FrozenError::RecipeOutOfRange)?;
                let x = topo.vertices[vid].point;
                // Constraint set = topological adjacency ∪ geometric
                // incidence. The union matters: after a boolean, a rim
                // vertex may carry half-edges of only one of its two
                // defining faces (the other keeps an untrimmed seam loop),
                // so loop membership alone under-constrains it.
                let mut incident: BTreeSet<usize> = adjacent.get(&vid).cloned().unwrap_or_default();
                for (sidx, surface) in brep.geometry.surfaces.iter().enumerate() {
                    if let Some(res) = surface_residual(surface.as_ref(), &x) {
                        if res < INCIDENCE_TOL {
                            incident.insert(sidx);
                        }
                    }
                }
                let mut rows = Vec::new();
                for &sidx in &incident {
                    let surface = brep.geometry.surfaces[sidx].as_ref();
                    rows.push(constraint_row(surface, seeding.get(sidx), &x)?);
                }
                let v = solve_vertex_velocity(&rows)?;
                positions.push(x);
                velocities.push(v);
            }
            NodeRecipe::SurfaceUv { face, u, v } => {
                let surface_index = {
                    let face_id = *ci
                        .faces
                        .get(face as usize)
                        .ok_or(FrozenError::RecipeOutOfRange)?;
                    topo.faces[face_id].surface_index
                };
                if let std::collections::hash_map::Entry::Vacant(e) = lifted.entry(face) {
                    let surface = brep.geometry.surfaces[surface_index].as_ref();
                    e.insert(lift_surface(surface, seeding.get(surface_index))?);
                }
                let (p, vel) =
                    lifted[&face].evaluate_with_velocity(vcad_kernel_math::Point2::new(u, v));
                positions.push(p);
                velocities.push(vel);
            }
        }
    }

    Ok(SeamMesh {
        positions,
        velocities,
        triangles: plan.triangles.clone(),
        recipes: plan.nodes.clone(),
    })
}

/// Signed mesh volume via the divergence theorem, generic over the scalar
/// type — evaluate with `Dual<f64>` node positions to get the volume and
/// its θ-derivative in one pass (and `Dual<Dual<f64>>` for second
/// derivatives, for free, later).
pub fn mesh_volume<S: Scalar>(positions: &[tang::Point3<S>], triangles: &[[u32; 3]]) -> S {
    let mut six_v = S::ZERO;
    for t in triangles {
        let a = &positions[t[0] as usize];
        let b = &positions[t[1] as usize];
        let c = &positions[t[2] as usize];
        six_v += a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x);
    }
    six_v / S::from_f64(6.0)
}

/// Volume and dV/dθ of a seam mesh: nodes are packed into `Dual<f64>`
/// points (real = position, dual = velocity) and pushed through the generic
/// volume integral, so `dV/dθ = Σ_i (∂V/∂x_i) · (dx_i/dθ)` falls out of
/// dual arithmetic.
pub fn volume_with_derivative(seam: &SeamMesh) -> (f64, f64) {
    let pts: Vec<tang::Point3<Dual<f64>>> = seam
        .positions
        .iter()
        .zip(&seam.velocities)
        .map(|(p, v)| {
            tang::Point3::new(
                Dual::new(p.x, v.x),
                Dual::new(p.y, v.y),
                Dual::new(p.z, v.z),
            )
        })
        .collect();
    let vol = mesh_volume(&pts, &seam.triangles);
    (vol.real, vol.dual)
}
