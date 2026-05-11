# phyz migration notes

This document records the Rapier3D → phyz migration for
`crates/vcad-kernel-physics`. The physics crate now wraps
[phyz](https://github.com/ecto/phyz), an in-house articulated-dynamics engine
that lives next to vcad as a sibling repo (`../phyz`).

## Why phyz

- Pure Rust, no C/C++ dependencies, no SIMD-only paths — clean WASM target.
- Reduced workspace dependency footprint (no more `rapier3d` / `parry3d` /
  `nalgebra`).
- Differentiable: phyz provides analytical Jacobians (`phyz-diff`) for future
  gradient-based control and parameter identification.
- Multi-physics roadmap (particles, fluids, EM) we may want later.

## phyz API survey

### Joint support

| vcad `JointKind` | phyz `JointType` | Notes |
|------------------|------------------|-------|
| `Revolute`       | `Revolute` (alias `Hinge`) | 1 DOF, rotates about `axis`. |
| `Prismatic`      | `Prismatic` (alias `Slide`) | 1 DOF, translates along `axis`. |
| `Fixed`          | `Fixed` | 0 DOF rigid attachment. |
| `Ball`           | `Spherical` (alias `Ball`) | 3 DOF, stored as axis-angle in `q`. |
| `Cylindrical`    | Two stacked phyz joints: a `Revolute` body inside a `Prismatic` body. | phyz has no first-class cylindrical joint, so the vcad cylindrical joint is realized as an extra fictitious body. Anchor offsets are split between the parent and child sides of the chain. |

phyz also exposes a `Free` 6-DOF joint which is what we use for the root of
free-floating instances that aren't attached to ground.

### Collision / geometry

phyz uses `phyz::Geometry`:
- `Geometry::Box { half_extents: Vec3 }`
- `Geometry::Sphere { radius: f64 }`
- `Geometry::Capsule { half_height: f64, radius: f64 }`
- `Geometry::Mesh { vertices: Vec<Vec3>, faces: Vec<[usize; 3]> }`
- `Geometry::Plane { normal: Vec3 }`

Broad phase is `sweep_and_prune`; narrow phase is GJK distance + EPA penetration
in `phyz-collision`. Contacts are penalty-based (`phyz-contact`), MuJoCo-style
soft contacts with stiffness/damping/friction in `ContactMaterial`.

Gaps vs. Rapier:
- No convex hull primitive — we map both `ConvexHull` and `TriMesh` strategies
  to `Geometry::Mesh` (faces only). For ground we keep `Geometry::Box` derived
  from the AABB.
- No continuous collision detection (CCD). Rapier had it; we do not rely on it
  for any current vcad feature.
- No compound colliders. Each rigid body in vcad already corresponds to a
  single mesh, so this is a non-issue.

### Integrator and determinism

- Featherstone ABA (`phyz::aba_with_external_forces`) for forward dynamics.
- Semi-implicit Euler with fixed dt (we use `1.0 / 240.0` like before).
- Pure-Rust, deterministic across architectures within IEEE-754 tolerances.
  No parallel solver or runtime SIMD branching, so cross-machine determinism
  is _better_ than Rapier's was.

### World / stepping API

`ModelBuilder` -> `Model` (immutable topology, masses, joint metadata) plus a
mutable `State { q, v, ctrl, time, body_xform, qfrc_external }`.

Per step:

```text
forward_kinematics(&model, &mut state);            // updates body_xform
let a = aba_with_external_forces(&model, &state, &tau, &fext);
state.v += a * dt;
state.q  = integrate(model, state.q, state.v, dt);
state.time += dt;
```

This is significantly thinner than Rapier's `PhysicsPipeline::step(...)` —
no separate broadphase/narrowphase/CCD pipeline objects.

### Mass properties

phyz needs `SpatialInertia` (mass + COM + 3×3 inertia tensor about COM) per
body. vcad supplies these from two sources, preferring the first when present:

1. URDF `<inertial>` blocks plumbed through `vcad_ir::InertialProperties`.
2. Mass estimated from mesh volume × density and an axis-aligned inertia
   approximation around the mesh bounding box.

phyz itself doesn't compute inertia from geometry — it accepts whatever the
caller hands it.

## vcad API delta

The crate name stays `vcad-kernel-physics`. Public API (`PhysicsWorld`,
`RobotEnv`, `Action`, `Observation`, `JointState`, `MotorTarget`,
`MotorMode`, `ColliderStrategy`, `PhysicsError`) is unchanged from the Rapier
era. Internals were rewritten to call phyz.

Behavioral differences callers may notice:

- Contact stability: phyz uses penalty-based soft contacts; Rapier used an
  impulse-based solver. Penetration during sustained loads can be slightly
  larger; tune `ContactMaterial::stiffness` if it matters for a task.
- Joint-limit handling: phyz joint limits are advisory in `Joint::limits` and
  are clamped at the integrator; Rapier enforced them as hard constraints
  through the solver. Aggressive control inputs may briefly overshoot a limit.
- Cylindrical joints now occupy two phyz body slots, so any code that walked
  `Model::bodies()` by index would see a different mapping. The `PhysicsWorld`
  hides this — external callers go through the joint/instance id maps and
  are not affected.
- Determinism is now byte-stable across x86_64/aarch64 builds within a single
  phyz release. Rapier's parallel solver did not guarantee this.

## Downstream surfaces

| Crate / package | Touched? | Notes |
|-----------------|----------|-------|
| `crates/vcad-kernel-physics` | yes | Internals fully on phyz. |
| `crates/vcad-kernel-wasm` | no | Re-exports types unchanged; recompiles. |
| `crates/vcad-kernel` | no | Did not re-export Rapier types. |
| `packages/engine/src/physics.ts` | doc-comment only | Mentioned Rapier in a comment. |
| `packages/mcp` | no | gym tools call `PhysicsWorld` and `RobotEnv`; API unchanged. |
| `mecheval/graders` | no | Suite C graders run rollouts through `RobotEnv`. |

## Workspace dependency status

`cargo tree --workspace | rg -i 'rapier|parry'` returns nothing. There are no
`rapier3d`, `rapier3d-f64`, `parry3d`, or `parry3d-f64` references anywhere in
the workspace (excluding `node_modules/`, which contains vendored Three.js
example helpers that vcad does not load).
