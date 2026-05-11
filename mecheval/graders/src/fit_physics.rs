//! Suite F (Fit) physics checks — `gravity_hold` and `pull_force`.
//!
//! These checks drive a minimal phyz simulation with two bodies:
//!
//! - Host: fixed body, mesh collider from `HostGeometry::mesh`.
//! - Accessory (candidate): free-floating body, mesh collider from
//!   the candidate `.vcad`.
//!
//! Both bodies are attached directly to the world (`parent = -1`). The
//! accessory's joint transform places it at its as-designed pose
//! relative to the host (the candidate is authored in the host's frame).
//!
//! Gravity is applied in the direction declared by the task; `pull_force`
//! adds a constant external force on the accessory along the declared
//! direction.
//!
//! The pass criterion is the same for both: the accessory's world-frame
//! translation between t=0 and t=duration_sec stays under `max_drift_mm`.
//! No rotation is measured; we'd rather err on the side of "the cap
//! settled and tilted" being a pass.
//!
//! Units note: phyz is metric (m, kg, s). The kernel meshes are in mm;
//! `vcad_kernel_physics::colliders` converts internally. All check
//! tolerances on the grader side are in mm.
//!
//! Backed by phyz ≥ 0.3, which ships:
//!
//! - `contact_forces_implicit` — implicit-damping penalty contacts that
//!   are unconditionally stable across dt, stiffness, and damping.
//! - Correct body-pair force signs (force on body i is opposite the
//!   normal; force on body j is along the normal).
//! - Contact forces applied at the contact point with the torque
//!   component (`τ = r × F`) so offset contacts produce rotation.
//! - NaN-robust broad phase.
//!
//! With those guarantees, F-suite tasks can tighten `max_drift_mm` to
//! single millimetres for retention checks.

use crate::blob::CheckOutcome;
use crate::fit::HostGeometry;
use phyz::collision::{epa_penetration_rot, gjk_distance_rot, sweep_and_prune, Collision, AABB};
use phyz::contact::contact_forces_implicit;
use phyz::math::{Mat3, SpatialInertia, SpatialTransform, SpatialVec, Vec3};
use phyz::model::ModelBuilder;
use phyz::{aba_with_external_forces, forward_kinematics};
use phyz::{ContactMaterial, Geometry};
use serde_json::json;
use std::panic::{catch_unwind, AssertUnwindSafe};
use vcad_kernel::vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel::Solid;
use vcad_kernel_physics::colliders::{estimate_mass, mesh_to_collider, ColliderStrategy};

/// Mesh density used to estimate accessory mass. 1200 kg/m³ approximates
/// a typical 3D-printed thermoplastic (PLA ≈ 1240). Tweakable if any task
/// needs a specific material; left fixed across F-suite for now so the
/// pass/fail boundary is reproducible.
const DEFAULT_ACCESSORY_DENSITY: f64 = 1200.0;

/// Default host mass; only matters as a sanity placeholder since the host
/// is a fixed body.
const DEFAULT_HOST_MASS_KG: f64 = 100.0;

/// Tessellation density for fit physics. Matches `fit::FIT_TESSELLATION_SEGMENTS`
/// so candidate/host mesh quality is consistent across all F-suite checks.
const FIT_PHYS_SEGMENTS: u32 = 64;

/// Fixed dt for the fit simulation. phyz's implicit contact solve is
/// stable at any dt; we pick 1/2000 to keep the trajectory smooth for
/// short rollouts.
const SIM_DT: f64 = 1.0 / 2000.0;

/// Contact parameters. With `contact_forces_implicit` we can use the
/// stock defaults — no over-damping workaround needed.
fn fit_material() -> ContactMaterial {
    ContactMaterial::default()
}

/// `gravity_hold`: simulate gravity for `duration_sec`, measure drift.
pub fn check_gravity_hold(
    candidate: &Solid,
    host: &HostGeometry,
    _host_mass_kg: f64,
    gravity_dir: [f64; 3],
    duration_sec: f64,
    max_drift_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        simulate_drop(candidate, host, gravity_dir, duration_sec, None)
    }));
    match result {
        Ok(Ok(drift_m)) => {
            let drift_mm = drift_m * 1000.0;
            let pass = drift_mm <= max_drift_mm;
            (
                if pass {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail
                },
                json!({
                    "drift_mm": drift_mm,
                    "max_drift_mm": max_drift_mm,
                    "duration_sec": duration_sec,
                    "gravity_dir": gravity_dir,
                }),
            )
        }
        Ok(Err(e)) => (
            CheckOutcome::Fail,
            json!({ "reason": "physics setup failed", "error": e }),
        ),
        Err(_) => (
            CheckOutcome::Fail,
            json!({ "reason": "physics simulation panicked" }),
        ),
    }
}

/// `pull_force`: apply `force_n` newtons along `direction` to the
/// accessory, simulate for `duration_sec`, measure drift.
pub fn check_pull_force(
    candidate: &Solid,
    host: &HostGeometry,
    force_n: f64,
    direction: [f64; 3],
    duration_sec: f64,
    max_drift_mm: f64,
) -> (CheckOutcome, serde_json::Value) {
    // Pull force pulls *against* gravity-style settling; we still run
    // with gravity ON (downward) so the accessory has a contact under
    // load. Tasks that want a horizontal pull can specify gravity_dir
    // implicitly via the world frame.
    let gravity_dir = [0.0, 0.0, -1.0];
    let dir_mag =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if dir_mag < 1e-9 {
        return (
            CheckOutcome::Fail,
            json!({ "reason": "pull direction is zero-length" }),
        );
    }
    let unit = [
        direction[0] / dir_mag,
        direction[1] / dir_mag,
        direction[2] / dir_mag,
    ];
    let pull = [unit[0] * force_n, unit[1] * force_n, unit[2] * force_n];

    let result = catch_unwind(AssertUnwindSafe(|| {
        simulate_drop(candidate, host, gravity_dir, duration_sec, Some(pull))
    }));
    match result {
        Ok(Ok(drift_m)) => {
            let drift_mm = drift_m * 1000.0;
            let pass = drift_mm <= max_drift_mm;
            (
                if pass {
                    CheckOutcome::Pass
                } else {
                    CheckOutcome::Fail
                },
                json!({
                    "drift_mm": drift_mm,
                    "max_drift_mm": max_drift_mm,
                    "duration_sec": duration_sec,
                    "force_n": force_n,
                    "direction": direction,
                }),
            )
        }
        Ok(Err(e)) => (
            CheckOutcome::Fail,
            json!({ "reason": "physics setup failed", "error": e }),
        ),
        Err(_) => (
            CheckOutcome::Fail,
            json!({ "reason": "physics simulation panicked" }),
        ),
    }
}

/// Build a 2-body phyz model (host fixed, accessory free), simulate
/// for `duration_sec`, return the accessory's world-frame translation
/// magnitude in meters. `external_world_force_n` is an optional constant
/// force on the accessory, expressed in world frame, in newtons.
fn simulate_drop(
    candidate: &Solid,
    host: &HostGeometry,
    gravity_dir: [f64; 3],
    duration_sec: f64,
    external_world_force_n: Option<[f64; 3]>,
) -> Result<f64, String> {
    let cand_mesh = candidate.to_mesh(FIT_PHYS_SEGMENTS);
    if cand_mesh.vertices.is_empty() || cand_mesh.indices.is_empty() {
        return Err("candidate mesh is empty".into());
    }
    if host.mesh.vertices.is_empty() || host.mesh.indices.is_empty() {
        return Err("host mesh is empty".into());
    }

    // phyz's narrow-phase falls back to `(pos_j - pos_i).normalize()` for
    // the contact normal. When both bodies' frame origins coincide at
    // world (0,0,0) — which would otherwise be a natural starting state —
    // the normal becomes NaN. We anchor each body's frame at its mesh
    // bbox centre, and store the geometry in body-local coordinates,
    // so the two frame origins are physically separated from the start.
    let host_centroid_mm = mesh_bbox_center_mm(&host.mesh);
    let cand_centroid_mm = mesh_bbox_center_mm(&cand_mesh);

    let host_local = translate_mesh(&host.mesh, host_centroid_mm);
    let cand_local = translate_mesh(&cand_mesh, cand_centroid_mm);

    // Use TriMesh colliders so non-convex hosts (a stepped shaft, a
    // C-channel bracket, …) are represented faithfully rather than as
    // one giant AABB. We pair this with our own EPA-based contact-depth
    // pass below (see `find_contacts_with_epa_depth`) — phyz's stock
    // `find_contacts` uses GJK's -1.0 penetrating-sentinel as the depth,
    // which causes 0.02mm contacts to read as 1m and launch the body.
    let host_geom = mesh_to_collider(&host_local, ColliderStrategy::TriMesh, "fit_host")
        .map_err(|e| format!("host collider build failed: {}", e))?;
    let cand_geom = mesh_to_collider(&cand_local, ColliderStrategy::TriMesh, "fit_accessory")
        .map_err(|e| format!("accessory collider build failed: {}", e))?;

    let host_inertia = bbox_inertia_about_origin(&host_local, DEFAULT_HOST_MASS_KG);
    let acc_mass = estimate_mass(&cand_local, DEFAULT_ACCESSORY_DENSITY);
    let acc_inertia = bbox_inertia_about_origin(&cand_local, acc_mass);

    // Body frame placement in world (metres).
    let host_pos = mm_vec_to_m(host_centroid_mm);
    let cand_pos = mm_vec_to_m(cand_centroid_mm);

    // Gravity magnitude is 9.81 m/s² along `gravity_dir` (which should be
    // a unit vector). We normalise defensively.
    let g_mag = (gravity_dir[0] * gravity_dir[0]
        + gravity_dir[1] * gravity_dir[1]
        + gravity_dir[2] * gravity_dir[2])
        .sqrt()
        .max(1e-9);
    let gravity = Vec3::new(
        9.81 * gravity_dir[0] / g_mag,
        9.81 * gravity_dir[1] / g_mag,
        9.81 * gravity_dir[2] / g_mag,
    );

    // Each body's frame is centred on its mesh bbox centre so the two
    // origins are physically separated and the contact normal is well
    // defined even when accessory + host start in contact.
    let host_xform = SpatialTransform::new(Mat3::identity(), host_pos);
    let cand_xform = SpatialTransform::new(Mat3::identity(), cand_pos);
    let model_builder = ModelBuilder::new()
        .gravity(gravity)
        .dt(SIM_DT)
        .add_fixed_body("host", -1, host_xform, host_inertia)
        .add_free_body("accessory", -1, cand_xform, acc_inertia);
    let mut model = model_builder.build();
    // Patch geometry on the bodies (the builder's `add_fixed_body` /
    // `add_free_body` doesn't accept geometry directly).
    model.bodies[0].geometry = Some(host_geom);
    model.bodies[1].geometry = Some(cand_geom);

    let mut state = model.default_state();

    // Initial forward kinematics → populates body_xform. For a free body
    // attached to world at `parent_to_joint`, `body_xform.pos` is the
    // body frame's world-frame position (see phyz/tests/integration.rs
    // `ball_drop_with_contacts` for the convention).
    let (xforms, _) = forward_kinematics(&model, &state);
    state.body_xform = xforms;
    let initial_pos = state.body_xform[1].pos;

    let n_steps = ((duration_sec / SIM_DT).round() as usize).max(1);
    let materials: Vec<ContactMaterial> = (0..model.bodies.len()).map(|_| fit_material()).collect();

    let pull = external_world_force_n.map(|f| Vec3::new(f[0], f[1], f[2]));

    // Per-body effective contact mass. Host is fixed → INFINITY (phyz
    // treats this as immovable in the implicit contact solve).
    let masses = vec![f64::INFINITY, acc_mass];

    for _ in 0..n_steps {
        let geometries: Vec<Option<Geometry>> =
            model.bodies.iter().map(|b| b.geometry.clone()).collect();
        // phyz's `find_contacts` returns penetration_depth = -gjk_distance,
        // and phyz's GJK uses -1.0 as a "penetrating" sentinel (depth
        // refinement is supposed to come from EPA). We do that EPA pass
        // ourselves so contact_forces_implicit sees true penetration
        // depth — without it a 0.02mm contact reads as 1m of penetration
        // and the cube launches at hundreds of m/s.
        let contacts = find_contacts_with_epa_depth(&state, &geometries);

        // Per-body world-frame spatial velocities for the contact solve.
        // Host has ndof=0 → zero. Accessory's free joint has v layout
        // [wx, wy, wz, vx, vy, vz]; we need (angular, linear) SpatialVec.
        let host_vel = SpatialVec::zero();
        let cand_vel = if state.v.len() >= 6 {
            SpatialVec::new(
                Vec3::new(state.v[0], state.v[1], state.v[2]),
                Vec3::new(state.v[3], state.v[4], state.v[5]),
            )
        } else {
            SpatialVec::zero()
        };
        let body_vels = [host_vel, cand_vel];

        let mut ext = contact_forces_implicit(
            &contacts,
            &state,
            &materials,
            Some(&body_vels),
            &masses,
            SIM_DT,
        );

        if let Some(p) = pull {
            // External force on the accessory body in world frame, applied
            // at the body's frame origin (zero torque component).
            if ext.len() >= 2 {
                ext[1] = ext[1] + SpatialVec::new(Vec3::zeros(), p);
            }
        }

        let qdd = aba_with_external_forces(&model, &state, Some(&ext));

        for i in 0..state.v.len() {
            state.v[i] += qdd[i] * SIM_DT;
        }
        // Free-joint integration: q layout is [x, y, z, wx, wy, wz] but
        // v layout is [wx, wy, wz, vx, vy, vz]. Translation is driven by
        // linear velocity (v[3..6] → q[0..3]); the exponential-coord
        // rotation by angular velocity (v[0..3] → q[3..6]). The fixed
        // host body has ndof=0, so it contributes nothing here.
        let free_q_off = model.q_offsets[1];
        let free_v_off = model.v_offsets[1];
        state.q[free_q_off] += state.v[free_v_off + 3] * SIM_DT;
        state.q[free_q_off + 1] += state.v[free_v_off + 4] * SIM_DT;
        state.q[free_q_off + 2] += state.v[free_v_off + 5] * SIM_DT;
        state.q[free_q_off + 3] += state.v[free_v_off] * SIM_DT;
        state.q[free_q_off + 4] += state.v[free_v_off + 1] * SIM_DT;
        state.q[free_q_off + 5] += state.v[free_v_off + 2] * SIM_DT;
        state.time += SIM_DT;

        let (xforms, _) = forward_kinematics(&model, &state);
        state.body_xform = xforms;
    }

    let final_pos = state.body_xform[1].pos;
    let dx = final_pos.x - initial_pos.x;
    let dy = final_pos.y - initial_pos.y;
    let dz = final_pos.z - initial_pos.z;
    Ok((dx * dx + dy * dy + dz * dz).sqrt())
}

/// Translate a [`phyz::Geometry`] (the `model::Geometry` re-export) into
/// the parallel [`phyz::collision::Geometry`] type that GJK/EPA accept.
fn to_collision_geometry(g: &Geometry) -> phyz::collision::Geometry {
    match g {
        Geometry::Sphere { radius } => phyz::collision::Geometry::Sphere { radius: *radius },
        Geometry::Capsule { radius, length } => phyz::collision::Geometry::Capsule {
            radius: *radius,
            length: *length,
        },
        Geometry::Box { half_extents } => phyz::collision::Geometry::Box {
            half_extents: *half_extents,
        },
        Geometry::Cylinder { radius, height } => phyz::collision::Geometry::Cylinder {
            radius: *radius,
            height: *height,
        },
        Geometry::Mesh { vertices, faces } => phyz::collision::Geometry::Mesh {
            vertices: vertices.clone(),
            faces: faces.clone(),
        },
        Geometry::Plane { normal } => phyz::collision::Geometry::Plane { normal: *normal },
    }
}

/// Run broad-phase + GJK + EPA over the model's bodies, returning
/// contacts with TRUE penetration depths. phyz's stock `find_contacts`
/// uses `penetration_depth = -gjk_distance` where GJK returns a -1.0
/// sentinel when penetrating, so its "depth" is unreliable.
fn find_contacts_with_epa_depth(
    state: &phyz::model::State,
    geometries: &[Option<Geometry>],
) -> Vec<Collision> {
    let aabbs: Vec<AABB> = geometries
        .iter()
        .enumerate()
        .map(|(i, g)| {
            if let Some(geom) = g {
                let cg = to_collision_geometry(geom);
                let pos = state.body_xform[i].pos;
                let rot = state.body_xform[i].rot;
                if !pos.x.is_finite() || !pos.y.is_finite() || !pos.z.is_finite() {
                    return AABB::new(Vec3::zeros(), Vec3::zeros());
                }
                AABB::from_geometry(&cg, &pos, &rot)
            } else {
                AABB::new(Vec3::zeros(), Vec3::zeros())
            }
        })
        .collect();

    let pairs = sweep_and_prune(&aabbs);
    let mut contacts = Vec::with_capacity(pairs.len());

    for (i, j) in pairs {
        let (gi, gj) = match (&geometries[i], &geometries[j]) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let cgi = to_collision_geometry(gi);
        let cgj = to_collision_geometry(gj);
        let xi = &state.body_xform[i];
        let xj = &state.body_xform[j];
        let dist = gjk_distance_rot(&cgi, &cgj, &xi.pos, &xj.pos, &xi.rot, &xj.rot);
        if dist >= 0.0 {
            continue;
        }
        // Penetrating — get true depth + normal from EPA.
        let (depth, mut normal) =
            match epa_penetration_rot(&cgi, &cgj, &xi.pos, &xj.pos, &xi.rot, &xj.rot) {
                Some(r) => r,
                None => continue,
            };
        if !depth.is_finite() || depth <= 0.0 {
            continue;
        }
        let n_norm = normal.norm();
        if n_norm > 1e-9 {
            normal *= 1.0 / n_norm;
        } else {
            // Fall back to centre-line direction if EPA degenerated.
            let co = xj.pos - xi.pos;
            let co_norm = co.norm();
            normal = if co_norm > 1e-9 {
                co * (1.0 / co_norm)
            } else {
                Vec3::z()
            };
        }
        let contact_point = (xi.pos + xj.pos) * 0.5;
        contacts.push(Collision {
            body_i: i,
            body_j: j,
            contact_point,
            contact_normal: normal,
            penetration_depth: depth,
        });
    }
    contacts
}

/// Bbox centre of a mesh in millimetres.
fn mesh_bbox_center_mm(mesh: &TriangleMesh) -> [f64; 3] {
    let mut min = [f64::INFINITY, f64::INFINITY, f64::INFINITY];
    let mut max = [f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for v in mesh.vertices.chunks(3) {
        for i in 0..3 {
            if (v[i] as f64) < min[i] {
                min[i] = v[i] as f64;
            }
            if (v[i] as f64) > max[i] {
                max[i] = v[i] as f64;
            }
        }
    }
    [
        0.5 * (min[0] + max[0]),
        0.5 * (min[1] + max[1]),
        0.5 * (min[2] + max[2]),
    ]
}

/// Convert a millimetre [x, y, z] to a phyz `Vec3` in metres.
fn mm_vec_to_m(mm: [f64; 3]) -> Vec3 {
    Vec3::new(mm[0] / 1000.0, mm[1] / 1000.0, mm[2] / 1000.0)
}

/// Return a copy of `mesh` with every vertex translated by `-offset_mm`.
/// Used so that the body frame for fit-physics sits at the mesh bbox
/// centre rather than the world origin.
fn translate_mesh(mesh: &TriangleMesh, offset_mm: [f64; 3]) -> TriangleMesh {
    let mut out = mesh.clone();
    for chunk in out.vertices.chunks_mut(3) {
        chunk[0] -= offset_mm[0] as f32;
        chunk[1] -= offset_mm[1] as f32;
        chunk[2] -= offset_mm[2] as f32;
    }
    out
}

/// Spatial inertia for a mesh whose bbox is centred on the origin: mass
/// at the origin (COM = (0,0,0)) with principal moments from a uniform-
/// density box of the mesh's extents. Mesh assumed to be in mm.
fn bbox_inertia_about_origin(mesh: &TriangleMesh, mass: f64) -> SpatialInertia {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in mesh.vertices.chunks(3) {
        let x = v[0] as f64 / 1000.0;
        let y = v[1] as f64 / 1000.0;
        let z = v[2] as f64 / 1000.0;
        if x < min.x {
            min.x = x;
        }
        if y < min.y {
            min.y = y;
        }
        if z < min.z {
            min.z = z;
        }
        if x > max.x {
            max.x = x;
        }
        if y > max.y {
            max.y = y;
        }
        if z > max.z {
            max.z = z;
        }
    }
    let dx = (max.x - min.x).max(1e-6);
    let dy = (max.y - min.y).max(1e-6);
    let dz = (max.z - min.z).max(1e-6);
    let ixx = mass / 12.0 * (dy * dy + dz * dz);
    let iyy = mass / 12.0 * (dx * dx + dz * dz);
    let izz = mass / 12.0 * (dx * dx + dy * dy);
    let inertia = Mat3::new(ixx, 0.0, 0.0, 0.0, iyy, 0.0, 0.0, 0.0, izz);
    SpatialInertia::new(mass, Vec3::zeros(), inertia)
}

/// Axis-aligned bbox-based spatial inertia: mass concentrated at the
/// mesh bbox centre, principal moments from a uniform-density box of
/// the same extents. Mirrors the fallback in `vcad-kernel-physics::world`.
/// Mesh assumed to be in mm. Retained for reference; the recentred path
/// uses [`bbox_inertia_about_origin`] instead.
#[allow(dead_code)]
fn bbox_inertia(mesh: &TriangleMesh, mass: f64) -> SpatialInertia {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in mesh.vertices.chunks(3) {
        let x = v[0] as f64 / 1000.0;
        let y = v[1] as f64 / 1000.0;
        let z = v[2] as f64 / 1000.0;
        if x < min.x {
            min.x = x;
        }
        if y < min.y {
            min.y = y;
        }
        if z < min.z {
            min.z = z;
        }
        if x > max.x {
            max.x = x;
        }
        if y > max.y {
            max.y = y;
        }
        if z > max.z {
            max.z = z;
        }
    }
    let dx = (max.x - min.x).max(1e-6);
    let dy = (max.y - min.y).max(1e-6);
    let dz = (max.z - min.z).max(1e-6);
    let com = Vec3::new(
        0.5 * (min.x + max.x),
        0.5 * (min.y + max.y),
        0.5 * (min.z + max.z),
    );
    let ixx = mass / 12.0 * (dy * dy + dz * dz);
    let iyy = mass / 12.0 * (dx * dx + dz * dz);
    let izz = mass / 12.0 * (dx * dx + dy * dy);
    let inertia = Mat3::new(ixx, 0.0, 0.0, 0.0, iyy, 0.0, 0.0, 0.0, izz);
    SpatialInertia::new(mass, com, inertia)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluate_vcad;
    use crate::fit::aggregate_candidate;
    use crate::task::InputFrame;

    fn cube_vcad(size: f64, offset: [f64; 3]) -> String {
        format!(
            r#"{{"version":"0.1","nodes":{{
                "1":{{"id":1,"op":{{"type":"Cube","size":{{"x":{s},"y":{s},"z":{s}}}}}}},
                "2":{{"id":2,"op":{{"type":"Translate","child":1,"offset":{{"x":{ox},"y":{oy},"z":{oz}}}}}}}
            }},"materials":{{}},"part_materials":{{}},"roots":[{{"root":2,"material":"default"}}]}}"#,
            s = size,
            ox = offset[0],
            oy = offset[1],
            oz = offset[2],
        )
    }

    fn plate_vcad(sx: f64, sy: f64, sz: f64, offset: [f64; 3]) -> String {
        format!(
            r#"{{"version":"0.1","nodes":{{
                "1":{{"id":1,"op":{{"type":"Cube","size":{{"x":{sx},"y":{sy},"z":{sz}}}}}}},
                "2":{{"id":2,"op":{{"type":"Translate","child":1,"offset":{{"x":{ox},"y":{oy},"z":{oz}}}}}}}
            }},"materials":{{}},"part_materials":{{}},"roots":[{{"root":2,"material":"default"}}]}}"#,
            sx = sx,
            sy = sy,
            sz = sz,
            ox = offset[0],
            oy = offset[1],
            oz = offset[2],
        )
    }

    fn make_host(raw: &str) -> HostGeometry {
        let snap = evaluate_vcad(raw);
        let solid = aggregate_candidate(&snap).expect("host solid");
        let mesh = solid.to_mesh(FIT_PHYS_SEGMENTS);
        HostGeometry {
            solid,
            mesh,
            frame: InputFrame {
                origin: [0.0, 0.0, 0.0],
                axis: [0.0, 0.0, 1.0],
            },
        }
    }

    /// 30mm cube resting on a thick plate. With phyz ≥ 0.3's implicit
    /// contact solve the cube settles within a few millimetres rather
    /// than launching, so we assert real settling here.
    #[test]
    fn cube_on_plate_settles_under_gravity() {
        let host = make_host(&plate_vcad(60.0, 60.0, 8.0, [-30.0, -30.0, 0.0]));
        let snap = evaluate_vcad(&cube_vcad(30.0, [-15.0, -15.0, 8.5]));
        let cand = aggregate_candidate(&snap).expect("cube solid");
        let drift_m = simulate_drop(&cand, &host, [0.0, 0.0, -1.0], 0.5, None).expect("sim runs");
        assert!(
            drift_m < 0.005,
            "expected < 5mm drift on plate, got {}m",
            drift_m
        );
    }

    /// Cube placed far above the plate, no plate — it falls into the
    /// void. Used as the negative control. Free fall over 0.5s under
    /// 9.81 m/s² is exactly 1.226m; we sanity-check we're in that ballpark.
    #[test]
    fn cube_in_void_falls_under_gravity() {
        // No host plate at all — only a tiny far-away plate the cube
        // can never reach.
        let host = make_host(&plate_vcad(1.0, 1.0, 1.0, [500.0, 500.0, -500.0]));
        let snap = evaluate_vcad(&cube_vcad(30.0, [-15.0, -15.0, 100.0]));
        let cand = aggregate_candidate(&snap).expect("cube solid");
        let drift_m = simulate_drop(&cand, &host, [0.0, 0.0, -1.0], 0.5, None).expect("sim runs");
        // Expect ~1.226m. Allow a wide window to absorb numerical drift.
        assert!(
            drift_m > 0.8 && drift_m < 1.6,
            "expected ~1.226m of free fall, got {}m",
            drift_m
        );
    }
}
