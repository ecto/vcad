# XLeRobot — vendored robot description

XLeRobot: an IKEA RÅSKOG cart carrying two 6-DOF SO-101 arms and a pan/tilt
head — a low-cost dual-arm mobile manipulator. URDF plus the meshes it
references, vendored so the simulation samples render and simulate with no
external checkout.

| | |
|---|---|
| Upstream | <https://github.com/Vector-Wangel/XLeRobot> |
| Commit | `3d14695e40c9c68229c0aacffca6053c75cd3eb6` |
| Source path | `simulation/Maniskill/assets/xlerobot/` |
| License | Apache-2.0 — see [LICENSE](LICENSE) |

## What is here

`xlerobot.urdf` and the **29** meshes it references (4.8 MB), byte-identical to
upstream. Upstream also ships `xlerobot_front.urdf` (a camera-placement
variant) and a separate MuJoCo asset tree; neither is vendored.

Five of the meshes are `.ply` gripper-jaw collision parts. vcad's importer
takes geometry from `<visual>`, so they are unreferenced by the imported
document — they are vendored anyway to keep the URDF's references complete.

## Importing

```bash
cargo run -p vcad-cli -- import-urdf \
  third_party/xlerobot/xlerobot.urdf \
  examples/xlerobot.vcad \
  --relative-meshes
```

`--relative-meshes` is what makes the result committable: without it the
importer writes absolute paths that only resolve on the machine that ran the
import.

Note there is **no `--floating-base`**, unlike the Booster K1. See below.

## Reading it in simulation

```bash
# does it build, and what are the DOF?
cargo run --release -p vcad-kernel-physics --example xlerobot_probe -- examples/xlerobot.vcad

# closed-loop PD control of both arms, with tracking error
cargo run --release -p vcad-sim --example xlerobot_reach -- examples/xlerobot.vcad
```

17 actuated DOF: 3 base + 12 arm + 2 head. Observation dimension 86. Roughly
20k steps/s single-threaded at dt = 1/200 s with 4 substeps.

## Three things to know before trusting a number out of this model

Upstream drives this URDF through ManiSkill/SAPIEN, which supplies actuator
limits from its own controller config and recomputes mass properties it does
not like. vcad instead simulates what the URDF actually says. Three
consequences, in descending order of how badly they will bite:

### 1. Every arm joint declares `effort="0"` — the arms are inert

All twelve arm joints (`Rotation`, `Pitch`, `Elbow`, `Wrist_Pitch`,
`Wrist_Roll`, `Jaw`, and their `_2` twins) carry `effort="0" velocity="0"`. A
zero effort limit is a hard saturation at zero torque, so as imported the arms
swing under gravity but no control input moves them. Measured: a **40 N·m**
command moves the `Elbow` by exactly **0.0000°**.

The importer warns about this on every import. It does not rewrite the value —
an effort limit is a declared physical claim, and guessing one fabricates
torque the description does not authorize.

`crates/vcad-sim/examples/xlerobot_reach.rs` supplies real limits: **2.94 N·m**,
the 12 V stall torque (30 kg·cm) of the Feetech STS3215 that every SO-101 joint
uses. Upstream's own config corroborates it — it sets `gripper_force_limit =
2.8` for the `Jaw` joints, the same servo and the one place a physical figure
was written down. Its `arm_force_limit = 250` is a SAPIEN drive-saturation
number, not a servo: 250 N·m on a 148 g forearm.

With 2.94 N·m the arm tracks a six-joint pose to **0.000°** steady-state error;
starved to 0.02 N·m the same reach misses by **90.3°**.

### 2. `base_link` is 70.13 kg

The URDF's authored inertial for the cart body is 70.129 kg, against a real
XLeRobot of roughly 10 kg. Everything else is plausible — the arm links are
0.02–0.19 kg and their inertia tensors are consistent with the meshes.

This is left **as upstream wrote it**. ManiSkill simulates the same 70 kg, so
changing it here would make vcad disagree with the reference implementation on
base acceleration for the same commanded force. Override it at load time if you
want the physical cart; do not assume the shipped number is the real robot.

### 3. The base is planar, not floating — the robot cannot tip

Mobility is a chain of three joints on a world-welded dummy link: prismatic X,
prismatic Y, continuous yaw. The cart cannot leave the ground plane, cannot
tip, and never touches the ground collider; the RÅSKOG wheel meshes are
decoration on `base_link`. There are no wheel joints and no rolling contact.

This is upstream's modelling choice and it is a reasonable one for a
manipulation benchmark — but it means balance, tip-over and traction questions
are outside what this model can answer, and that a height- or tilt-based
termination condition (the kind a humanoid like the Booster K1 needs) can never
fire. Importing with `--floating-base` would *not* fix this; it would add a
6-DOF joint above a robot that still has no wheels to stand on.

## Eleven links carry no `<inertial>`

The four gripper tips, two arm cameras and five head-camera frames declare no
inertial block. All eleven are pure coordinate frames on `fixed` joints, which
is ordinary URDF practice — they are not a defect. vcad derives their mass
properties from geometry (a 1 cm placeholder cube for the frames that have no
mesh either), so they contribute negligible mass rather than a singular one.
