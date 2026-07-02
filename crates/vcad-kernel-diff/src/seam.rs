//! Assembly of the seam: per-node positions and velocities for a frozen
//! plan, plus dual-valued quantities of interest built on top of them.

use std::collections::{BTreeSet, HashMap};

use tang::Dual;
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{
    canonical_index, mesh_volume, refine_boundary_point, signature_with_index, FrozenError,
    FrozenPlan, NodeRecipe,
};
use vcad_kernel_topo::VertexId;

use crate::{
    constraint_row, lift_surface, solve_vertex_velocity, surface_residual, tangency_rows,
    DiffError, ParamSeeding,
};

/// Incidence tolerance (mm) for treating a topology vertex as lying on a
/// surface. Must sit above the placement error of boolean/trim vertices on
/// their defining surfaces (analytic intersections are ~machine precision;
/// sampled-fallback intersections can be off by ~1e-6 mm — a missed
/// incidence silently *loses* a constraint, so the tolerance errs high) and
/// far below feature separation.
const INCIDENCE_TOL: f64 = 1e-4;

/// Tolerance (mm) for the structural capture-time-B-rep check: every
/// resolved node must land on the plan's recorded base position.
const ANCHOR_TOL: f64 = 1e-3;

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
/// matches entities geometrically instead). This contract is enforced, not
/// just documented: every resolved node is checked against the plan's
/// recorded base position, so a rebuilt B-rep whose (order-nondeterministic)
/// enumeration permutes entities fails with `CorrespondenceLost` instead of
/// silently binding recipes to the wrong geometry.
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
    if plan.base_positions.len() != plan.nodes.len() {
        return Err(FrozenError::RecipeOutOfRange.into());
    }

    let topo = &brep.topology;

    // Adjacent surface indices per topology vertex (distinct indices only),
    // plus the set of surface indices actually referenced by the solid's
    // faces — the geometric incidence scan below is restricted to the
    // latter, so surfaces a boolean left in the store without a bounding
    // face can never contribute a spurious constraint row.
    let mut adjacent: HashMap<VertexId, BTreeSet<usize>> = HashMap::new();
    let mut referenced: BTreeSet<usize> = BTreeSet::new();
    for &face_id in &ci.faces {
        let face = &topo.faces[face_id];
        referenced.insert(face.surface_index);
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

    let anchor_check = |node: usize, p: &Point3, anchor: &Point3| -> Result<(), DiffError> {
        let d2 = (*p - *anchor).norm_squared();
        if d2 > ANCHOR_TOL * ANCHOR_TOL {
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
    for (i, recipe) in plan.nodes.iter().enumerate() {
        match *recipe {
            NodeRecipe::TopoVertex { vertex } => {
                let vid = *ci
                    .vertices
                    .get(vertex as usize)
                    .ok_or(FrozenError::RecipeOutOfRange)?;
                let x = topo.vertices[vid].point;
                anchor_check(i, &x, &plan.base_positions[i])?;
                // Constraint set = topological adjacency ∪ geometric
                // incidence. The union matters: after a boolean, a rim
                // vertex may carry half-edges of only one of its two
                // defining faces (the other keeps an untrimmed seam loop),
                // so loop membership alone under-constrains it.
                let mut incident: BTreeSet<usize> = adjacent.get(&vid).cloned().unwrap_or_default();
                for &sidx in &referenced {
                    let surface = brep.geometry.surfaces[sidx].as_ref();
                    if let Some(res) = surface_residual(surface, &x) {
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
                // Tangency completion: a curved surface resting on an
                // incident plane duplicates the plane's row instead of
                // pinning the vertex tangentially; its tangent-curve rows
                // carry the missing directions (a rounded-cube corner
                // vertex slides along the support face as the fillet
                // radius grows). Looping over all (plane, surface) pairs
                // over-generates safely: non-tangent pairs contribute no
                // rows at all, and redundant rows from multiple tangent
                // contacts are orthogonalized away by the solver after a
                // consistency check — disagreement is a hard error, never
                // a silent average.
                for &pidx in &incident {
                    let plane = brep.geometry.surfaces[pidx].as_ref();
                    let Some(p) = plane.as_any().downcast_ref::<vcad_kernel_geom::Plane>() else {
                        continue;
                    };
                    let n = *p.normal_dir.as_ref();
                    for &sidx in &incident {
                        if sidx == pidx {
                            continue;
                        }
                        let surface = brep.geometry.surfaces[sidx].as_ref();
                        rows.extend(tangency_rows(n, surface, seeding.get(sidx), &x)?);
                    }
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
                anchor_check(i, &p, &plan.base_positions[i])?;
                positions.push(p);
                velocities.push(vel);
            }
            NodeRecipe::Boundary { face_a, face_b, .. } => {
                // A trim-boundary node: position from the two-surface
                // Newton refinement; velocity from the same two-row
                // implicit system used for rim topology vertices, with the
                // tangential DOF frozen. This is what makes the node track
                // the moving trim regardless of which surface carries θ.
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
                let rows = vec![
                    constraint_row(sa, seeding.get(sidx_a), &x)?,
                    constraint_row(sb, seeding.get(sidx_b), &x)?,
                ];
                let v = solve_vertex_velocity(&rows)?;
                positions.push(x);
                velocities.push(v);
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

/// Volume and dV/dθ of a seam mesh: nodes are packed into `Dual<f64>`
/// points (real = position, dual = velocity) and pushed through the shared
/// generic volume integral
/// ([`vcad_kernel_tessellate::frozen::mesh_volume`] — the same integrator
/// the FD oracle uses at `f64`), so
/// `dV/dθ = Σ_i (∂V/∂x_i) · (dx_i/dθ)` falls out of dual arithmetic.
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
