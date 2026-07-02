//! M5 — reverse mode: the adjoint seam.
//!
//! Forward mode ([`crate::evaluate_with_sensitivity`]) costs one seam pass
//! **per parameter**: each θ_k needs its own [`ParamSeeding`] and its own
//! velocity field. Reverse mode transposes the seam once: given a mesh
//! functional's gradient `∂J/∂x_i` (from [`crate::volume_gradient`], a
//! physics adjoint, or any other source), [`evaluate_with_pullback`]
//! accumulates the gradient of `J` with respect to every surface's
//! *seed slots* — its translation velocity and radius rate. Contracting
//! those cotangents against any number of seedings
//! ([`MeshCotangents::contract`]) is then a handful of dot products per
//! parameter, with **no further seam evaluations**:
//!
//! ```text
//! dJ/dθ_k = Σ_s ⟨cotangent_s, seeds_k(s)⟩
//! ```
//!
//! The transpose never re-derives a formula the forward path owns. Node
//! velocities are linear in the seed values, through two linear maps:
//!
//! - **Lift-bridge nodes** (`SurfaceUv`): the map's columns are read off by
//!   evaluating the *forward* lift with unit basis seeds (unit translations
//!   and a unit radius rate), so the adjoint is exact by construction.
//! - **Implicit nodes** (`TopoVertex`, `Boundary`): the vertex solve is
//!   linear in the row rhs vector ([`crate::row_pullbacks`] returns
//!   `m_j = ∂ẋ/∂rhs_j`), and each row's rhs is linear in its owning
//!   surface's seeds — columns again read off by re-materializing the row
//!   with unit basis seeds through the same constructors the forward pass
//!   uses ([`RowSource`] keeps the correspondence).
//!
//! Basis probing happens strictly **per row / per surface**, never through
//! a joint vertex solve: seeding one surface at a time through the full
//! solve would trip the consistency check whenever a moving surface has
//! duplicate copies in a boolean's store (each copy's row would contradict
//! the others'), which is precisely the case `seed_where` exists for.

use std::collections::{BTreeMap, HashMap};

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{refine_boundary_point, FrozenError, FrozenPlan, NodeRecipe};

use crate::seam::{
    assemble_vertex_rows, checked_index, incidence_context, vertex_incident_surfaces, RowSource,
};
use crate::{
    lift_surface, row_pullbacks, ConstraintRow, DiffError, DualSurface, ParamSeeding, SurfaceSeed,
};
use vcad_kernel_geom::SurfaceKind;

/// Gradient of a mesh functional `J` with respect to one surface's seed
/// slots.
///
/// Every scalar shape parameter a surface kind exposes gets its **own**
/// slot — the torus in particular has two independent radii, so overloading
/// a single `radius` slot would price a major- and a minor-radius parameter
/// against each other. The [`ScalarSlot`] enum names them; the translation
/// slot (ℝ³) is shared by every kind.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SurfaceCotangent {
    /// ∂J/∂(translation velocity): contract with a
    /// [`SurfaceSeed::Translate`]'s velocity.
    pub translate: Vec3,
    /// ∂J/∂(radius rate): contract with a [`SurfaceSeed::CylinderRadius`] /
    /// [`SurfaceSeed::SphereRadius`] rate. Zero for kinds without a radius.
    pub radius: f64,
    /// ∂J/∂(half-angle rate): contract with a [`SurfaceSeed::ConeAngle`]
    /// rate. Zero for non-cone kinds.
    pub cone_angle: f64,
    /// ∂J/∂(major-radius rate): contract with a
    /// [`SurfaceSeed::TorusMajorRadius`] rate. Zero for non-torus kinds.
    pub torus_major: f64,
    /// ∂J/∂(minor-radius rate): contract with a
    /// [`SurfaceSeed::TorusMinorRadius`] rate. Zero for non-torus kinds.
    pub torus_minor: f64,
}

/// A named scalar (non-translation) seed slot of a surface cotangent. Each
/// surface kind reports which slots it has via [`scalar_bases`]; both the
/// lift-bridge and the row pullbacks accumulate into them uniformly, so
/// adding a kind's scalar parameter is one arm here plus one field above.
#[derive(Debug, Clone, Copy)]
enum ScalarSlot {
    Radius,
    ConeAngle,
    TorusMajor,
    TorusMinor,
}

impl SurfaceCotangent {
    fn add_scalar(&mut self, slot: ScalarSlot, value: f64) {
        match slot {
            ScalarSlot::Radius => self.radius += value,
            ScalarSlot::ConeAngle => self.cone_angle += value,
            ScalarSlot::TorusMajor => self.torus_major += value,
            ScalarSlot::TorusMinor => self.torus_minor += value,
        }
    }
}

/// Per-surface cotangents of a mesh functional: the output of one
/// reverse-mode seam pass, contractable against any number of parameter
/// seedings.
#[derive(Debug, Clone, Default)]
pub struct MeshCotangents {
    per_surface: BTreeMap<usize, SurfaceCotangent>,
}

impl MeshCotangents {
    /// The cotangent of the surface at `surface_index` (zero if the
    /// functional is insensitive to it).
    pub fn get(&self, surface_index: usize) -> SurfaceCotangent {
        self.per_surface
            .get(&surface_index)
            .copied()
            .unwrap_or_default()
    }

    /// `dJ/dθ = Σ_s ⟨cotangent_s, seeds(s)⟩` for one parameter's seeding.
    ///
    /// The seeding must be one the forward path would accept for the same
    /// B-rep: valid seed kinds per surface, and every copy of a moving
    /// surface seeded together (as [`ParamSeeding::seed_where`] does).
    /// Forward evaluation is where invalid seedings are *detected*; the
    /// contraction is a plain bilinear form and cannot check them.
    pub fn contract(&self, seeding: &ParamSeeding) -> f64 {
        let mut total = 0.0;
        for (&sidx, cot) in &self.per_surface {
            for seed in seeding.get(sidx) {
                total += match *seed {
                    SurfaceSeed::Translate { velocity } => cot.translate.dot(velocity),
                    SurfaceSeed::CylinderRadius { rate } | SurfaceSeed::SphereRadius { rate } => {
                        cot.radius * rate
                    }
                    SurfaceSeed::ConeAngle { rate } => cot.cone_angle * rate,
                    SurfaceSeed::TorusMajorRadius { rate } => cot.torus_major * rate,
                    SurfaceSeed::TorusMinorRadius { rate } => cot.torus_minor * rate,
                };
            }
        }
        total
    }
}

/// The unit basis seeds for a surface kind's scalar (non-translation) slots,
/// each paired with the [`ScalarSlot`] its column accumulates into. Plane
/// has none; cylinder/sphere have a radius; cone a half-angle; torus two
/// independent radii. Probing happens per basis seed, so a kind with several
/// scalars (the torus) prices each independently.
fn scalar_bases(kind: SurfaceKind) -> &'static [(SurfaceSeed, ScalarSlot)] {
    match kind {
        SurfaceKind::Cylinder => &[(
            SurfaceSeed::CylinderRadius { rate: 1.0 },
            ScalarSlot::Radius,
        )],
        SurfaceKind::Sphere => &[(SurfaceSeed::SphereRadius { rate: 1.0 }, ScalarSlot::Radius)],
        SurfaceKind::Cone => &[(SurfaceSeed::ConeAngle { rate: 1.0 }, ScalarSlot::ConeAngle)],
        SurfaceKind::Torus => &[
            (
                SurfaceSeed::TorusMajorRadius { rate: 1.0 },
                ScalarSlot::TorusMajor,
            ),
            (
                SurfaceSeed::TorusMinorRadius { rate: 1.0 },
                ScalarSlot::TorusMinor,
            ),
        ],
        _ => &[],
    }
}

fn translate_bases() -> [Vec3; 3] {
    [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ]
}

/// Re-materialize one vertex-system row from its source with a single
/// basis seed and read its rhs — the row's seed-Jacobian column.
fn row_rhs_column(
    brep: &BRepSolid,
    source: &RowSource,
    seed: SurfaceSeed,
    x: &Point3,
) -> Result<f64, DiffError> {
    match *source {
        RowSource::Constraint { surface } => {
            let s = brep.geometry.surfaces[surface].as_ref();
            Ok(crate::constraint_row(s, &[seed], x)?.rhs)
        }
        RowSource::Tangency {
            plane_normal,
            surface,
            index,
        } => {
            let s = brep.geometry.surfaces[surface].as_ref();
            let rows = crate::tangency_rows(plane_normal, s, &[seed], x)?;
            Ok(rows[index].rhs)
        }
    }
}

fn source_surface(source: &RowSource) -> usize {
    match *source {
        RowSource::Constraint { surface } | RowSource::Tangency { surface, .. } => surface,
    }
}

/// Accumulate `λ · ∂rhs/∂seeds` of one row into its surface's cotangent.
fn accumulate_row(
    brep: &BRepSolid,
    source: &RowSource,
    lambda: f64,
    x: &Point3,
    cots: &mut BTreeMap<usize, SurfaceCotangent>,
) -> Result<(), DiffError> {
    if lambda == 0.0 {
        return Ok(());
    }
    let sidx = source_surface(source);
    let kind = brep.geometry.surfaces[sidx].surface_type();
    let mut translate = Vec3::new(0.0, 0.0, 0.0);
    for (axis, e) in translate_bases().iter().enumerate() {
        let col = row_rhs_column(brep, source, SurfaceSeed::Translate { velocity: *e }, x)?;
        match axis {
            0 => translate.x = col,
            1 => translate.y = col,
            _ => translate.z = col,
        }
    }
    let cot = cots.entry(sidx).or_default();
    cot.translate += translate * lambda;
    for &(seed, slot) in scalar_bases(kind) {
        let col = row_rhs_column(brep, source, seed, x)?;
        cot.add_scalar(slot, col * lambda);
    }
    Ok(())
}

/// Reverse-mode seam evaluation: pull a mesh functional's gradient
/// `∂J/∂x` back to per-surface seed cotangents in **one pass**.
///
/// `brep` must be the capture-time B-rep, exactly as for
/// [`crate::evaluate_with_sensitivity`] — the same signature and anchor
/// checks are enforced. `mesh_gradient` is indexed like the plan's nodes
/// (one gradient per node, e.g. from [`crate::volume_gradient`]).
///
/// The returned cotangents satisfy, for every seeding the forward path
/// accepts on this B-rep,
///
/// ```text
/// cotangents.contract(seeding) == contract_sensitivity(forward(seeding), mesh_gradient)
/// ```
///
/// so `n` parameters cost one pullback plus `n` dot products instead of
/// `n` forward passes.
pub fn evaluate_with_pullback(
    brep: &BRepSolid,
    plan: &FrozenPlan,
    mesh_gradient: &[Vec3],
) -> Result<MeshCotangents, DiffError> {
    if mesh_gradient.len() != plan.nodes.len() {
        return Err(DiffError::GradientLengthMismatch {
            expected: plan.nodes.len(),
            got: mesh_gradient.len(),
        });
    }
    let ci = checked_index(brep, plan)?;
    let topo = &brep.topology;
    let ctx = incidence_context(brep, &ci.faces);
    let empty = ParamSeeding::new();

    // Forward-lifted surfaces with one unit basis seed each, cached per
    // (face slot, basis): 0..=2 are unit translations, 3 the radius rate.
    let mut lifted: HashMap<(u32, u8), DualSurface> = HashMap::new();

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

    let mut cots: BTreeMap<usize, SurfaceCotangent> = BTreeMap::new();

    for (i, recipe) in plan.nodes.iter().enumerate() {
        let w = mesh_gradient[i];
        match *recipe {
            NodeRecipe::TopoVertex { vertex } => {
                let vid = *ci
                    .vertices
                    .get(vertex as usize)
                    .ok_or(FrozenError::RecipeOutOfRange)?;
                let x = topo.vertices[vid].point;
                anchor_check(i, &x, &plan.base_positions[i])?;
                let incident = vertex_incident_surfaces(brep, &ctx, vid, &x);
                let sourced = assemble_vertex_rows(brep, &incident, &empty, &x)?;
                let rows: Vec<ConstraintRow> = sourced.iter().map(|(_, row)| *row).collect();
                let m = row_pullbacks(&rows);
                for ((source, _), mj) in sourced.iter().zip(&m) {
                    accumulate_row(brep, source, w.dot(*mj), &x, &mut cots)?;
                }
            }
            NodeRecipe::SurfaceUv { face, u, v } => {
                let surface_index = {
                    let face_id = *ci
                        .faces
                        .get(face as usize)
                        .ok_or(FrozenError::RecipeOutOfRange)?;
                    topo.faces[face_id].surface_index
                };
                let kind = brep.geometry.surfaces[surface_index].surface_type();
                let uv = vcad_kernel_math::Point2::new(u, v);
                // Basis index layout per face slot: 0..=2 unit translations,
                // then one per scalar seed the kind exposes (in
                // `scalar_bases` order).
                let mut basis_velocity = |basis: u8| -> Result<(Point3, Vec3), DiffError> {
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        lifted.entry((face, basis))
                    {
                        let seed = match basis {
                            0..=2 => SurfaceSeed::Translate {
                                velocity: translate_bases()[basis as usize],
                            },
                            _ => scalar_bases(kind)[(basis - 3) as usize].0,
                        };
                        let surface = brep.geometry.surfaces[surface_index].as_ref();
                        e.insert(lift_surface(surface, &[seed])?);
                    }
                    Ok(lifted[&(face, basis)].evaluate_with_velocity(uv))
                };
                let (p, vx) = basis_velocity(0)?;
                anchor_check(i, &p, &plan.base_positions[i])?;
                let (_, vy) = basis_velocity(1)?;
                let (_, vz) = basis_velocity(2)?;
                let cot = cots.entry(surface_index).or_default();
                cot.translate += Vec3::new(w.dot(vx), w.dot(vy), w.dot(vz));
                for (k, &(_, slot)) in scalar_bases(kind).iter().enumerate() {
                    let (_, vk) = basis_velocity(3 + k as u8)?;
                    cot.add_scalar(slot, w.dot(vk));
                }
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
                let sourced = [
                    (
                        RowSource::Constraint { surface: sidx_a },
                        crate::constraint_row(sa, &[], &x)?,
                    ),
                    (
                        RowSource::Constraint { surface: sidx_b },
                        crate::constraint_row(sb, &[], &x)?,
                    ),
                ];
                let rows: Vec<ConstraintRow> = sourced.iter().map(|(_, row)| *row).collect();
                let m = row_pullbacks(&rows);
                for ((source, _), mj) in sourced.iter().zip(&m) {
                    accumulate_row(brep, source, w.dot(*mj), &x, &mut cots)?;
                }
            }
        }
    }

    Ok(MeshCotangents { per_surface: cots })
}
