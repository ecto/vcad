//! Closed-loop control of the XLeRobot dual-arm mobile manipulator.
//!
//! ```bash
//! # import first (see third_party/xlerobot/README.md):
//! #   vcad import-urdf third_party/xlerobot/xlerobot.urdf examples/xlerobot.vcad --relative-meshes
//! cargo run --release -p vcad-sim --example xlerobot_reach -- examples/xlerobot.vcad
//! ```
//!
//! # What this robot is, in simulation terms
//!
//! XLeRobot is an IKEA RÅSKOG cart carrying two 6-DOF SO-101 arms and a
//! pan/tilt head. Its 17 actuated DOF are:
//!
//! | group   | joints                                             | count |
//! |---------|----------------------------------------------------|-------|
//! | base    | `root_x_axis_joint`, `root_y_axis_joint`, `root_z_rotation_joint` | 3 |
//! | arms    | `Rotation`/`Pitch`/`Elbow`/`Wrist_Pitch`/`Wrist_Roll`/`Jaw` ×2     | 12 |
//! | head    | `head_pan_joint`, `head_tilt_joint`                | 2 |
//!
//! The base is **not** a floating base: it is a planar chain (prismatic X,
//! prismatic Y, continuous yaw) rooted at a world-welded dummy link. The cart
//! therefore cannot tip, cannot leave the ground plane, and never touches the
//! ground collider — the wheel meshes are decoration on `base_link`. This is
//! the upstream ManiSkill modelling choice, and it makes the interesting
//! question *manipulation*, not balance. Nothing here needs the termination
//! conditions a humanoid like the Booster K1 does.
//!
//! # The actuator profile, and why it is not the URDF's
//!
//! `xlerobot.urdf` declares `effort="0" velocity="0"` on all twelve arm
//! joints. A zero effort limit is a hard saturation at zero torque, so the
//! arms as imported are inert: measured here, a 40 N·m command moves the
//! Elbow by exactly 0.0000°. Upstream never notices because ManiSkill takes
//! actuator limits from its own controller config and never reads the URDF's.
//!
//! So the limits have to come from somewhere. This example uses the physical
//! servo: every arm joint on an SO-101 is a Feetech **STS3215**, whose 12 V
//! stall torque is 30 kg·cm ≈ **2.94 N·m**. That number is corroborated by
//! upstream's own config, which sets `gripper_force_limit = 2.8` for the
//! `Jaw` joints — the same servo, the one place they wrote a physical figure.
//!
//! Deliberately *not* adopted: upstream's `arm_force_limit = 250` and
//! `arm_stiffness = 2e4`. Those are SAPIEN drive-saturation and stiffness
//! numbers chosen to make its implicit PD tracker stiff; 250 N·m on a 148 g
//! forearm is not a servo, and 2e4 in an explicit integrator is far past the
//! stability limit (this example asks [`RobotEnv::check_gain_stability`] and
//! reports what the timestep can actually carry).
//!
//! The head keeps the URDF's own limits (0.32 / 0.68 N·m) and the base keeps
//! its 100 N — those were filled in and are plausible.

use vcad_ir::{Document, JointKind};
use vcad_kernel_physics::{Action, EnvConfig, RobotEnv};

/// Feetech STS3215 stall torque at 12 V (30 kg·cm), in N·m.
const STS3215_STALL_NM: f64 = 2.94;

/// STS3215 no-load speed, ~45 rpm, in deg/s — what vcad's `velocity_limit`
/// wants. The URDF's `velocity="0"` is as unusable as its `effort="0"`.
const STS3215_NO_LOAD_DEG_S: f64 = 270.0;

/// The twelve arm joints, in URDF order. Both arms are the same SO-101.
const ARM_JOINTS: [&str; 12] = [
    "Rotation",
    "Pitch",
    "Elbow",
    "Wrist_Pitch",
    "Wrist_Roll",
    "Jaw",
    "Rotation_2",
    "Pitch_2",
    "Elbow_2",
    "Wrist_Pitch_2",
    "Wrist_Roll_2",
    "Jaw_2",
];

/// Give the arm joints the STS3215's real limits, replacing the URDF's zeros.
///
/// Returns the number of joints patched. Joints that already carry a nonzero
/// effort limit (head, base) are left exactly as the URDF declared them.
fn apply_actuator_profile(doc: &mut Document) -> usize {
    let mut patched = 0;
    for joint in doc.joints.iter_mut().flatten() {
        if !ARM_JOINTS.contains(&joint.id.as_str()) {
            continue;
        }
        if let JointKind::Revolute {
            effort_limit,
            velocity_limit,
            ..
        } = &mut joint.kind
        {
            if effort_limit.is_none_or(|v| v == 0.0) {
                *effort_limit = Some(STS3215_STALL_NM);
                patched += 1;
            }
            if velocity_limit.is_none_or(|v| v == 0.0) {
                *velocity_limit = Some(STS3215_NO_LOAD_DEG_S);
            }
        }
    }
    patched
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "examples/xlerobot.vcad".to_string());

    let json = std::fs::read_to_string(&path)?;
    let mut doc: Document = serde_json::from_str(&json)?;
    // The committed document stores mesh paths relative to its own directory.
    if let Some(dir) = std::path::Path::new(&path).parent() {
        vcad_eval::resolve_mesh_paths(&mut doc, dir);
    }

    let patched = apply_actuator_profile(&mut doc);
    println!(
        "actuator profile: patched {patched} inert arm joints to {STS3215_STALL_NM} N·m (STS3215)"
    );

    let cfg = EnvConfig {
        base_instance_id: Some("base_link_inst".to_string()),
        ..EnvConfig::default()
    };
    let dt = 1.0 / 200.0;
    let substeps = 4;
    let mut env = RobotEnv::new_with_config(
        doc,
        vec!["Fixed_Jaw_tip_inst".into(), "Fixed_Jaw_tip_2_inst".into()],
        Some(dt),
        Some(substeps),
        None,
        cfg,
    )?;
    println!(
        "env: {} actuated DOF, obs {}, dt {:.4}s x{} substeps\n",
        env.action_dim(),
        env.observation_dim(),
        dt,
        substeps
    );

    // --- gains -------------------------------------------------------------
    // No explicit gains are set. vcad's default PD law is inertia-scaled
    // (kp = I·omega^2, kd = 2·I·omega at omega = 20 rad/s), so omega·dt is
    // constant across joints regardless of how light the link is — stable by
    // construction. That matters a lot here: the reflected inertia of these
    // SO-101 links spans 1.4e-3 kg·m^2 at the Elbow down to 3.1e-5 at the
    // Jaw, a factor of 45. Any single hand-picked kp that tracks the Elbow
    // is far past the stability limit at the Jaw (measured: kp = 12 gives
    // omega·dt = 3.09 there, ten times over, and the arm flails).
    let ids: Vec<String> = env.actuated_joint_ids().to_vec();
    let warnings = env.check_gain_stability();
    if warnings.is_empty() {
        println!(
            "gain stability: all {} joints within omega*dt limit\n",
            ids.len()
        );
    } else {
        println!("gain stability: {} joint(s) over the limit", warnings.len());
        for w in &warnings {
            println!(
                "  {:22} kp={:.1} max_stable={:.1} omega_dt={:.3} (needs {} substeps)",
                w.joint_id, w.kp, w.max_stable_kp, w.omega_dt, w.min_substeps
            );
        }
        println!();
    }

    // --- index maps --------------------------------------------------------
    // Two different orderings, deliberately named apart: `act` indexes the
    // action vector (actuated joints only), `obs_at` indexes the observation
    // (all joints, Fixed included). Conflating them is the trap that
    // `joint_observation_index` exists to close.
    let act = |name: &str| ids.iter().position(|j| j == name).unwrap();
    let obs_at = {
        let map: Vec<(String, usize)> = ids
            .iter()
            .map(|id| (id.clone(), env.joint_observation_index(id).unwrap()))
            .collect();
        move |name: &str| map.iter().find(|(id, _)| id == name).unwrap().1
    };

    // --- task: hold the rest pose -----------------------------------------
    env.reset();
    let hold = vec![0.0; env.action_dim()];
    let mut obs = env.observe();
    for _ in 0..400 {
        let (o, _, _) = env.step(Action::PositionTarget(hold.clone()));
        obs = o;
    }
    let worst_hold = ARM_JOINTS
        .iter()
        .map(|j| obs.joint_positions[obs_at(j)].abs())
        .fold(0.0_f64, f64::max);
    println!("\nhold rest pose, 2.0 s: worst arm joint drift {worst_hold:.4} deg");

    // --- task: reach ------------------------------------------------------
    // Command a distinct pose per arm joint and measure steady-state tracking.
    // These sit inside every joint's URDF limit.
    let targets: [(&str, f64); 6] = [
        ("Rotation", 30.0),
        ("Pitch", 45.0),
        ("Elbow", 60.0),
        ("Wrist_Pitch", -25.0),
        ("Wrist_Roll", 40.0),
        ("Jaw", 20.0),
    ];
    let mut action = vec![0.0; env.action_dim()];
    for (name, deg) in targets {
        action[act(name)] = deg;
    }

    env.reset();
    let start = env.observe();
    let tcp0 = start.end_effector_poses[0];
    let mut obs = start;
    for _ in 0..600 {
        let (o, _, _) = env.step(Action::PositionTarget(action.clone()));
        obs = o;
    }
    let tcp1 = obs.end_effector_poses[0];

    println!("\nreach, 3.0 s of PD position control on the right arm:");
    println!(
        "  {:<14} {:>10} {:>10} {:>10}",
        "joint", "target", "reached", "error"
    );
    let mut worst = 0.0_f64;
    for (name, deg) in targets {
        let q = obs.joint_positions[obs_at(name)];
        let err = (q - deg).abs();
        worst = worst.max(err);
        println!("  {name:<14} {deg:>10.2} {q:>10.2} {err:>10.3}");
    }
    println!("  worst steady-state error: {worst:.3} deg");
    println!(
        "  right gripper TCP: ({:.4}, {:.4}, {:.4}) -> ({:.4}, {:.4}, {:.4}) m, moved {:.4} m",
        tcp0[0],
        tcp0[1],
        tcp0[2],
        tcp1[0],
        tcp1[1],
        tcp1[2],
        ((tcp1[0] - tcp0[0]).powi(2) + (tcp1[1] - tcp0[1]).powi(2) + (tcp1[2] - tcp0[2]).powi(2))
            .sqrt()
    );

    // The left arm was commanded to hold zero throughout — if it moved, the
    // two arms are coupled through the base and that is worth knowing.
    let left_drift = ["Rotation_2", "Pitch_2", "Elbow_2"]
        .iter()
        .map(|j| obs.joint_positions[obs_at(j)].abs())
        .fold(0.0_f64, f64::max);
    println!("  left arm (commanded to hold 0): worst drift {left_drift:.4} deg");

    println!(
        "\nany NaN: {}",
        obs.joint_positions.iter().any(|v| v.is_nan())
    );

    // --- control: is the actuator model actually load-bearing? -------------
    // Tracking to 0.000 deg on links this light is easy, and "easy" and
    // "not simulated at all" look identical in a results table. Re-run the
    // same reach with the servo starved to 0.02 N·m. If the effort limit is
    // being enforced the arm must now fail to hold the pose against gravity;
    // if the number came out identical, the torque budget was never in the
    // loop and the first table proved nothing.
    let starved = reach_with_effort(&path, 0.02, &targets)?;
    println!("\ncontrol — same reach with the servo starved to 0.02 N·m:");
    println!("  worst steady-state error: {starved:.3} deg (nominal 2.94 N·m gave {worst:.3})");
    if starved > worst + 1.0 {
        println!("  -> effort limit is enforced; the 2.94 N·m result is a real torque budget");
    } else {
        println!("  -> WARNING: starving the servo changed nothing — effort limit not in the loop");
    }

    Ok(())
}

/// Re-run the reach on a fresh env whose arm joints are capped at
/// `effort_nm`, returning the worst steady-state tracking error in degrees.
fn reach_with_effort(
    path: &str,
    effort_nm: f64,
    targets: &[(&str, f64)],
) -> Result<f64, Box<dyn std::error::Error>> {
    let json = std::fs::read_to_string(path)?;
    let mut doc: Document = serde_json::from_str(&json)?;
    if let Some(dir) = std::path::Path::new(path).parent() {
        vcad_eval::resolve_mesh_paths(&mut doc, dir);
    }
    for joint in doc.joints.iter_mut().flatten() {
        if !ARM_JOINTS.contains(&joint.id.as_str()) {
            continue;
        }
        if let JointKind::Revolute {
            effort_limit,
            velocity_limit,
            ..
        } = &mut joint.kind
        {
            *effort_limit = Some(effort_nm);
            *velocity_limit = Some(STS3215_NO_LOAD_DEG_S);
        }
    }
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
    let ids: Vec<String> = env.actuated_joint_ids().to_vec();
    let mut action = vec![0.0; env.action_dim()];
    for (name, deg) in targets {
        action[ids.iter().position(|j| j == name).unwrap()] = *deg;
    }
    env.reset();
    let mut obs = env.observe();
    for _ in 0..600 {
        let (o, _, _) = env.step(Action::PositionTarget(action.clone()));
        obs = o;
    }
    Ok(targets
        .iter()
        .map(|(name, deg)| {
            (obs.joint_positions[env.joint_observation_index(name).unwrap()] - deg).abs()
        })
        .fold(0.0_f64, f64::max))
}
