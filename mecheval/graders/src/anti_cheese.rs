//! Anti-cheese enforcement.
//!
//! The plan here is small: walk the task's [`AntiCheese`] block against
//! the candidate document + Suite-C assembly snapshot, and surface a
//! list of violations. Per-task structural constraints (min rigid-body
//! count, required tags, mass ceilings, etc.) live here; tool-call /
//! token-count ceilings live in `task.limits` and are enforced by the
//! harness.
//!
//! `joint_torque_ceiling_nm` is recorded but not double-checked here —
//! it's consumed by the `torque_budget` check.

use crate::eval::EvalSnapshot;
use crate::fit::{aggregate_candidate, load_host};
use crate::task::{AntiCheese, Task};
use std::path::Path;
use vcad_ir::{Document, JointKind};
use vcad_kernel::vcad_kernel_tessellate::TriangleMesh;
use vcad_kernel::Solid;
use vcad_kernel_physics::colliders::estimate_mass;

/// Result of running the anti-cheese pass.
#[derive(Debug, Default, Clone)]
pub struct AntiCheeseReport {
    /// Human-readable violation messages. Empty when nothing was tripped.
    pub violations: Vec<String>,
}

impl AntiCheeseReport {
    /// True iff any rule was violated.
    pub fn violated(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// Check every rule on `task.anti_cheese` against the candidate. The
/// document (when available) is preferred for assembly-shape checks; the
/// snapshot's solids are used for Suite-A solid-count and mass.
///
/// `task_dir` resolves Suite-F host geometry referenced by `inputs[]`.
/// Pass `Path::new(".")` if the task has no Fit anti-cheese rules.
pub fn enforce(task: &Task, snapshot: &EvalSnapshot, task_dir: &Path) -> AntiCheeseReport {
    let mut report = AntiCheeseReport::default();
    let rules = &task.anti_cheese;
    let doc = snapshot.doc.as_ref();

    if let Some(min) = rules.min_rigid_bodies {
        let count = doc
            .and_then(|d| d.instances.as_ref())
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        if count < min {
            report.violations.push(format!(
                "min_rigid_bodies: expected ≥{}, got {}",
                min, count
            ));
        }
    }

    if let Some(min) = rules.min_actuated_joints {
        let count = doc
            .and_then(|d| d.joints.as_ref())
            .map(|js| {
                js.iter()
                    .filter(|j| !matches!(j.kind, JointKind::Fixed))
                    .count() as u32
            })
            .unwrap_or(0);
        if count < min {
            report.violations.push(format!(
                "min_actuated_joints: expected ≥{}, got {}",
                min, count
            ));
        }
    }

    if let Some(max) = rules.max_solid_count {
        let count = snapshot.solids.len() as u32;
        if count > max {
            report
                .violations
                .push(format!("max_solid_count: expected ≤{}, got {}", max, count));
        }
    }

    if let Some(max_kg) = rules.max_total_mass_kg {
        let total = total_assembly_mass_kg(doc).unwrap_or(0.0);
        if total > max_kg {
            report.violations.push(format!(
                "max_total_mass_kg: expected ≤{}, got {:.4}",
                max_kg, total
            ));
        }
    }

    if !rules.required_links.is_empty() {
        let missing = missing_required_links(doc, &rules.required_links);
        if !missing.is_empty() {
            report.violations.push(format!(
                "required_links: missing tag/name for {:?}",
                missing
            ));
        }
    }

    let _consumed_by_torque_budget = rules.joint_torque_ceiling_nm;

    // ---- Fit-suite anti-cheese ----------------------------------------

    if let Some(min_v) = rules.min_accessory_volume_mm3 {
        let v = snapshot.aggregate_volume().unwrap_or(0.0);
        if v < min_v {
            report.violations.push(format!(
                "min_accessory_volume_mm3: expected ≥{}, got {:.3}",
                min_v, v
            ));
        }
    }

    let needs_host = rules.max_overlap_with_host_envelope_pct.is_some()
        || rules.must_enclose_host_centroid.is_some();

    if needs_host {
        match load_host(task, task_dir) {
            Ok(host) => {
                if let Some(max_pct) = rules.max_overlap_with_host_envelope_pct {
                    if let (Some(cand_solid), Some((c_lo, c_hi))) =
                        (aggregate_candidate(snapshot), snapshot.aggregate_bbox())
                    {
                        let (h_lo, h_hi) = host.solid.bounding_box();
                        let inter_lo = [
                            c_lo[0].max(h_lo[0]),
                            c_lo[1].max(h_lo[1]),
                            c_lo[2].max(h_lo[2]),
                        ];
                        let inter_hi = [
                            c_hi[0].min(h_hi[0]),
                            c_hi[1].min(h_hi[1]),
                            c_hi[2].min(h_hi[2]),
                        ];
                        let inter_vol = (0..3)
                            .map(|i| (inter_hi[i] - inter_lo[i]).max(0.0))
                            .product::<f64>();
                        let host_vol = (0..3)
                            .map(|i| (h_hi[i] - h_lo[i]).max(0.0))
                            .product::<f64>();
                        if host_vol > 0.0 {
                            let pct = 100.0 * inter_vol / host_vol;
                            if pct > max_pct {
                                report.violations.push(format!(
                                    "max_overlap_with_host_envelope_pct: expected ≤{:.2}, got {:.2}",
                                    max_pct, pct
                                ));
                            }
                        }
                        let _ = cand_solid; // bbox already captured above
                    }
                }
                if let Some(must) = rules.must_enclose_host_centroid {
                    if let Some((c_lo, c_hi)) = snapshot.aggregate_bbox() {
                        let centroid = host.solid.center_of_mass();
                        let inside =
                            (0..3).all(|i| centroid[i] >= c_lo[i] && centroid[i] <= c_hi[i]);
                        if inside != must {
                            report.violations.push(format!(
                                "must_enclose_host_centroid: expected {}, accessory bbox {} host centroid {:?}",
                                must,
                                if inside { "encloses" } else { "does not enclose" },
                                centroid
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                report.violations.push(format!(
                    "host_geometry required by anti-cheese but could not load: {}",
                    e
                ));
            }
        }
    }

    report
}

/// Sum up estimated body masses across every assembly instance, using
/// the document's material density (defaulting to 1000 kg/m³). Returns
/// `None` for documents without assembly data.
fn total_assembly_mass_kg(doc: Option<&Document>) -> Option<f64> {
    let doc = doc?;
    let instances = doc.instances.as_ref()?;
    let part_defs = doc.part_defs.as_ref()?;
    let mut total = 0.0_f64;
    for inst in instances {
        let pd = match part_defs.get(&inst.part_def_id) {
            Some(p) => p,
            None => continue,
        };
        let mesh = match part_root_mesh(doc, pd.root) {
            Some(m) => m,
            None => continue,
        };
        let density = doc
            .materials
            .get(inst.material.as_deref().unwrap_or("default"))
            .and_then(|m| m.density)
            .unwrap_or(1000.0);
        total += estimate_mass(&mesh, density);
    }
    Some(total)
}

/// Tessellate a part's root primitive into a triangle mesh — mirrors
/// the lightweight evaluator inside `PhysicsWorld::evaluate_part`.
/// Returns `None` for non-primitive ops (the current task corpus only
/// uses primitives for Suite C bodies).
fn part_root_mesh(doc: &Document, root: vcad_ir::NodeId) -> Option<TriangleMesh> {
    let node = doc.nodes.get(&root)?;
    let solid = match &node.op {
        vcad_ir::CsgOp::Cube { size } => Solid::cube(size.x, size.y, size.z),
        vcad_ir::CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => Solid::cylinder(
            *radius,
            *height,
            if *segments == 0 { 32 } else { *segments },
        ),
        vcad_ir::CsgOp::Sphere { radius, segments } => {
            Solid::sphere(*radius, if *segments == 0 { 32 } else { *segments })
        }
        vcad_ir::CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => Solid::cone(
            *radius_bottom,
            *radius_top,
            *height,
            if *segments == 0 { 32 } else { *segments },
        ),
        // For non-primitive ops we conservatively skip; mass enforcement
        // is a structural ceiling, not a precision check, so under-
        // estimating is fine — it only ever lets *more* candidates
        // through, never fails them spuriously.
        _ => return None,
    };
    Some(solid.to_mesh(32))
}

/// Required tags missing from every instance's `tags` list (case-
/// insensitive). Falls back to instance `name`/`id` when no instance
/// in the doc carries any tag — older candidates predating the IR
/// `tags` field shouldn't fail spuriously.
fn missing_required_links(doc: Option<&Document>, required: &[String]) -> Vec<String> {
    let instances = match doc.and_then(|d| d.instances.as_ref()) {
        Some(v) => v,
        None => return required.to_vec(),
    };
    let any_tags = instances.iter().any(|i| !i.tags.is_empty());

    let mut missing = Vec::new();
    for req in required {
        let present = instances.iter().any(|i| {
            i.tags.iter().any(|t| t.eq_ignore_ascii_case(req))
                || (!any_tags
                    && (i.id.eq_ignore_ascii_case(req)
                        || i.name
                            .as_deref()
                            .is_some_and(|n| n.eq_ignore_ascii_case(req))))
        });
        if !present {
            missing.push(req.clone());
        }
    }
    missing
}

/// Convenience wrapper used by the grader: just the bool + message list.
pub fn run(task: &Task, snapshot: &EvalSnapshot, task_dir: &Path) -> (bool, Vec<String>) {
    if is_empty_rules(&task.anti_cheese) {
        return (false, Vec::new());
    }
    let r = enforce(task, snapshot, task_dir);
    (r.violated(), r.violations)
}

fn is_empty_rules(rules: &AntiCheese) -> bool {
    rules.min_rigid_bodies.is_none()
        && rules.min_actuated_joints.is_none()
        && rules.max_solid_count.is_none()
        && rules.max_total_mass_kg.is_none()
        && rules.joint_torque_ceiling_nm.is_none()
        && rules.required_links.is_empty()
        && rules.max_overlap_with_host_envelope_pct.is_none()
        && rules.min_accessory_volume_mm3.is_none()
        && rules.must_enclose_host_centroid.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::CheckSpec;
    use crate::eval::evaluate_vcad;
    use crate::task::{Limits, Suite};

    fn task_with(rules: AntiCheese) -> Task {
        Task {
            id: "ac-test".into(),
            suite: Suite::C,
            tier: "C".into(),
            title: "t".into(),
            prompt: "p".into(),
            inputs: vec![],
            checks: vec![CheckSpec::BodyValid],
            anti_cheese: rules,
            limits: Limits::default(),
            pass_k: 1,
            tags: vec![],
        }
    }

    /// One ground cube + one revolute child + a tip child, with `tags`
    /// covering both `base` and `tip`.
    fn reacher_vcad() -> String {
        r#"{
            "version":"0.1",
            "nodes":{
                "1":{"id":1,"name":"base_g","op":{"type":"Cube","size":{"x":100,"y":100,"z":50}}},
                "2":{"id":2,"name":"l1_g","op":{"type":"Cube","size":{"x":20,"y":20,"z":100}}},
                "3":{"id":3,"name":"l2_g","op":{"type":"Cube","size":{"x":20,"y":20,"z":100}}}
            },
            "materials":{},
            "part_materials":{},
            "roots":[],
            "partDefs":{
                "base":{"id":"base","root":1},
                "l1":{"id":"l1","root":2},
                "l2":{"id":"l2","root":3}
            },
            "instances":[
                {"id":"base_inst","partDefId":"base","name":"Base","tags":["base"]},
                {"id":"l1_inst","partDefId":"l1","name":"L1"},
                {"id":"l2_inst","partDefId":"l2","name":"Tip","tags":["tip"]}
            ],
            "joints":[
                {"id":"j1","parentInstanceId":"base_inst","childInstanceId":"l1_inst",
                 "parentAnchor":{"x":0,"y":0,"z":25},"childAnchor":{"x":0,"y":0,"z":-50},
                 "kind":{"type":"Revolute","axis":{"x":0,"y":1,"z":0},"limits":[-180,180]},
                 "state":0},
                {"id":"j2","parentInstanceId":"l1_inst","childInstanceId":"l2_inst",
                 "parentAnchor":{"x":0,"y":0,"z":50},"childAnchor":{"x":0,"y":0,"z":-50},
                 "kind":{"type":"Revolute","axis":{"x":0,"y":1,"z":0},"limits":[-180,180]},
                 "state":0}
            ],
            "groundInstanceId":"base_inst"
        }"#
        .to_string()
    }

    #[test]
    fn passes_when_rules_satisfied() {
        let task = task_with(AntiCheese {
            min_rigid_bodies: Some(3),
            min_actuated_joints: Some(2),
            max_total_mass_kg: Some(5.0),
            joint_torque_ceiling_nm: Some(5.0),
            required_links: vec!["base".into(), "tip".into()],
            ..Default::default()
        });
        let snap = evaluate_vcad(&reacher_vcad());
        let (violated, vs) = run(&task, &snap, std::path::Path::new("."));
        assert!(!violated, "violations: {:?}", vs);
    }

    #[test]
    fn flags_min_rigid_bodies() {
        let task = task_with(AntiCheese {
            min_rigid_bodies: Some(99),
            ..Default::default()
        });
        let snap = evaluate_vcad(&reacher_vcad());
        let (violated, vs) = run(&task, &snap, std::path::Path::new("."));
        assert!(violated);
        assert!(vs.iter().any(|m| m.contains("min_rigid_bodies")));
    }

    #[test]
    fn flags_min_actuated_joints() {
        let task = task_with(AntiCheese {
            min_actuated_joints: Some(5),
            ..Default::default()
        });
        let snap = evaluate_vcad(&reacher_vcad());
        let (violated, vs) = run(&task, &snap, std::path::Path::new("."));
        assert!(violated);
        assert!(vs.iter().any(|m| m.contains("min_actuated_joints")));
    }

    #[test]
    fn flags_required_links_missing() {
        let task = task_with(AntiCheese {
            required_links: vec!["foot".into(), "tip".into()],
            ..Default::default()
        });
        let snap = evaluate_vcad(&reacher_vcad());
        let (violated, vs) = run(&task, &snap, std::path::Path::new("."));
        assert!(violated);
        assert!(vs.iter().any(|m| m.contains("foot")));
        // 'tip' tag is present so it must NOT be reported as missing.
        assert!(!vs.iter().any(|m| m.contains("\"tip\"")));
    }

    #[test]
    fn flags_max_total_mass_kg() {
        let task = task_with(AntiCheese {
            max_total_mass_kg: Some(0.001),
            ..Default::default()
        });
        let snap = evaluate_vcad(&reacher_vcad());
        let (violated, vs) = run(&task, &snap, std::path::Path::new("."));
        assert!(violated, "expected mass violation, got vs={:?}", vs);
        assert!(vs.iter().any(|m| m.contains("max_total_mass_kg")));
    }

    #[test]
    fn empty_rules_short_circuits() {
        let task = task_with(AntiCheese::default());
        let snap = evaluate_vcad(&reacher_vcad());
        let (violated, vs) = run(&task, &snap, std::path::Path::new("."));
        assert!(!violated);
        assert!(vs.is_empty());
    }
}
