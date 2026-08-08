//! Sanity probe for the XLeRobot mobile-manipulator env.
//!
//! Usage: `cargo run --release -p vcad-kernel-physics --example xlerobot_probe -- <xlerobot.vcad> [mode]`
//! where mode is `passive` (default, zero torque) or `hold` (PD to rest pose).
//!
//! Unlike the K1 probe there is no falling to watch for: XLeRobot's base is a
//! planar chain (prismatic X, prismatic Y, continuous yaw), so the cart cannot
//! leave the ground plane and cannot tip. What this probe is looking for is
//! whether the arms are *controllable* — the upstream URDF declares
//! `effort="0"` on every arm joint.

use std::time::Instant;
use vcad_ir::Document;
use vcad_kernel_physics::{Action, EnvConfig, RobotEnv};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: xlerobot_probe <doc.vcad> [passive|hold]");
    let mode = std::env::args().nth(2).unwrap_or_else(|| "passive".into());
    let json = std::fs::read_to_string(&path)?;
    let mut doc: Document = serde_json::from_str(&json)?;
    // The committed document stores mesh paths relative to its own
    // directory; without this the camera links (visual-only, no authored
    // inertial) have no geometry to derive mass properties from.
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let n = vcad_eval::resolve_mesh_paths(&mut doc, dir);
        println!("resolved {n} mesh paths against {}", dir.display());
        for node in doc.nodes.values() {
            if let vcad_ir::CsgOp::MeshImport { path, .. } = &node.op {
                if !std::path::Path::new(path).exists() {
                    println!("  MISSING: {path}");
                }
            }
        }
    }

    let t0 = Instant::now();
    let cfg = EnvConfig {
        base_instance_id: Some("base_link_inst".to_string()),
        ..EnvConfig::default()
    };
    let mut env = RobotEnv::new_with_config(
        doc,
        vec!["Fixed_Jaw_tip_inst".into(), "Fixed_Jaw_tip_2_inst".into()],
        Some(1.0 / 200.0),
        Some(4),
        None,
        cfg,
    )?;
    println!("build: {:?}", t0.elapsed());
    println!(
        "action_dim={} obs_dim={} floating_base={}",
        env.action_dim(),
        env.observation_dim(),
        env.has_floating_base()
    );
    println!("actuated joints:");
    for id in env.actuated_joint_ids() {
        println!("  {id}");
    }

    if mode == "hold" {
        for id in env.actuated_joint_ids().to_vec() {
            env.set_joint_gains(&id, 40.0, 2.0);
        }
    }

    let obs = env.reset();
    println!("\nreset base_pose={:?}", obs.base_pose);
    for (i, p) in obs.end_effector_poses.iter().enumerate() {
        println!("  gripper {i}: pos=({:.4}, {:.4}, {:.4})", p[0], p[1], p[2]);
    }

    // Drive every actuated joint hard and see whether anything moves.
    let n = 200;
    let t1 = Instant::now();
    let mut last = obs;
    for i in 0..n {
        let action = if mode == "hold" {
            Action::PositionTarget(vec![0.3; env.action_dim()])
        } else {
            Action::Torque(vec![5.0; env.action_dim()])
        };
        let (o, _r, done) = env.step(action);
        last = o;
        if done {
            println!("terminated at step {i}");
            break;
        }
    }
    let dt = t1.elapsed();
    println!(
        "\n{n} steps in {:?} ({:.0} steps/s)",
        dt,
        n as f64 / dt.as_secs_f64()
    );

    println!("\nfinal joint positions (nonzero only):");
    let ids = env.actuated_joint_ids().to_vec();
    let mut moved = 0;
    for id in ids.iter() {
        // Observations are indexed over *all* joints (Fixed ones keep a zero
        // slot), not over the actuated ones the action vector uses. Indexing
        // this by the loop counter reads a neighbouring joint's angle —
        // `arm_base_joint` is Fixed and sits right before `Rotation`.
        let q = env
            .joint_observation_index(id)
            .and_then(|i| last.joint_positions.get(i).copied())
            .unwrap_or(0.0);
        if q.abs() > 1e-9 {
            println!("  {id:26} q={q:+.6}");
            moved += 1;
        }
    }
    println!("{moved}/{} joints moved", ids.len());
    println!(
        "any NaN: {}",
        last.joint_positions.iter().any(|v| v.is_nan())
    );

    Ok(())
}
