# Booster K1 — vendored robot description

The 22-DOF floating-base Booster K1 humanoid: URDF plus the STL meshes it
references, vendored so the simulation samples render and simulate with no
external checkout.

| | |
|---|---|
| Upstream | <https://github.com/BoosterRobotics/booster_assets> |
| Commit | `508cbee6ca9ae6fbc8c0b38dd58785a6f3fc61a2` |
| License | BSD 3-Clause — see [LICENSE](LICENSE) |
| Copyright | © 2025 Booster Robotics |

## What is here, and what is not

`K1_22dof_floating.urdf` and the **24** meshes it references (12 MB). Upstream
ships 52 meshes across five URDF variants; the rest are for variants this repo
does not use and are deliberately not vendored.

The floating-base variant is the one to import for locomotion and balance work:
it declares a 6-DOF root joint, so the robot can actually fall over. The
fixed-base `K1_22dof.urdf` is bolted to the world, which silently disables every
height and tilt termination — see `docs/native-sim-m0.md`.

## Regenerating

```bash
cargo run -p vcad-cli -- import-urdf \
  third_party/booster-k1/K1_22dof_floating.urdf \
  examples/k1-floating.vcad \
  --floating-base --spawn-height-mm 550 --relative-meshes
```

`--relative-meshes` is what makes the result committable: without it the
importer writes absolute paths that only resolve on the machine that ran the
import. Relative paths are resolved against the document's own directory when
it is loaded from disk.

## Why vendor at all

A URDF whose meshes are missing still *simulates* correctly — mass, centre of
mass and inertia come from the authored `<inertial>` blocks, and only the
collider falls back to a placeholder box. But it **renders nothing**, and in the
app that presents as an empty viewport with the Simulate affordance absent,
because a document whose every instance mesh is empty looks like a document with
no assembly at all. Vendoring is what makes the K1 a usable sample rather than a
headless fixture.
