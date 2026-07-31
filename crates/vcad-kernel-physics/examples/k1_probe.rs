//! Timing + sanity probe for the Booster K1 floating-base env.
//!
//! Usage: `cargo run --release -p vcad-kernel-physics --example k1_probe -- <k1.vcad> [mode]`
//! where mode is `passive` (default, zero torque) or `hold` (PD to rest pose).

use std::time::Instant;
use vcad_ir::Document;
use vcad_kernel_physics::{Action, EnvConfig, GroundConfig, RobotEnv, TerminationConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).expect("usage: k1_probe <doc.vcad>");
    let mode = std::env::args().nth(2).unwrap_or_else(|| "passive".into());
    let json = std::fs::read_to_string(&path)?;
    let doc: Document = serde_json::from_str(&json)?;

    let t0 = Instant::now();
    let cfg = EnvConfig {
        termination: Some(TerminationConfig {
            base_height_below: Some(0.2),
            base_tilt_above_deg: Some(80.0),
            terminate_on_joint_limit: false,
        }),
        base_instance_id: Some("Trunk_inst".to_string()),
        ..EnvConfig::default()
    };
    let mut env = RobotEnv::new_with_config(
        doc,
        vec!["left_foot_link_inst".into(), "right_foot_link_inst".into()],
        Some(std::env::var("DT").map_or(1.0 / 200.0, |v| v.parse().unwrap())),
        Some(std::env::var("SUB").map_or(4, |v| v.parse().unwrap())),
        if std::env::var("NO_GROUND").is_ok() {
            Some(GroundConfig::disabled())
        } else {
            None
        },
        cfg,
    )?;
    env.set_max_steps(2000);
    println!("build: {:?}", t0.elapsed());
    let act_ids: Vec<String> = env.actuated_joint_ids().to_vec();
    println!(
        "action_dim={} obs_dim={}",
        env.action_dim(),
        env.observation_dim()
    );
    if mode == "hold" {
        for id in &act_ids {
            let (kp, kd) = if id.contains("Hip") || id.contains("Knee") {
                (200.0, 5.0)
            } else if id.contains("Ankle") {
                (50.0, 1.0)
            } else {
                (40.0, 1.0)
            };
            env.set_joint_gains(id, kp, kd);
        }
    }

    let obs = env.reset();
    println!("reset base_pose={:?}", obs.base_pose);
    for (i, p) in obs.end_effector_poses.iter().enumerate() {
        println!("  foot {i}: pos=({:.4}, {:.4}, {:.4})", p[0], p[1], p[2]);
    }

    let n = 200;
    let t1 = Instant::now();
    for i in 0..n {
        let action = if mode.starts_with("hold") {
            Action::PositionTarget(vec![0.0; env.action_dim()])
        } else {
            Action::Torque(vec![0.0; env.action_dim()])
        };
        let r = env.step_full(action);
        if i < 15 || i % 10 == 0 || r.done {
            let f = r
                .observation
                .end_effector_poses
                .first()
                .copied()
                .unwrap_or([0.0; 7]);
            println!(
                "step {i:>3} t={:.3}s h={:.4} tilt={:>6.2} foot_z={:.4} done={} {:?}",
                (i + 1) as f64 * 4.0 / 200.0,
                r.info.base_height_m.unwrap_or(f64::NAN),
                r.info.base_tilt_deg.unwrap_or(f64::NAN),
                f[2],
                r.done,
                r.info.termination_reason
            );
        }
        if r.done {
            break;
        }
    }
    let el = t1.elapsed();
    println!(
        "{n} steps in {:?} => {:.1} steps/s",
        el,
        n as f64 / el.as_secs_f64()
    );
    Ok(())
}
