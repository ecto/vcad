# Unitree robot fixtures

Four URDF descriptors live in this directory for Unitree robots:

| File | Source | Geometry | DOF | Status |
|------|--------|----------|-----|--------|
| `unitree-g1.urdf` | `unitreerobotics/unitree_ros` (g1_23dof) | 50 STL meshes + 12 primitives | 23 | **meshes not vendored** |
| `unitree-go2.urdf` | `unitreerobotics/unitree_ros` (go2_description) | 17 meshes + 27 primitives | 12 actuated (41 total joints) | **meshes not vendored** |

> This table used to claim both files were hand-authored, primitives-only and
> self-contained, and listed two `*-official.urdf` variants that are not in this
> directory. Neither was true: at some point the hand-authored files were
> replaced by the upstream ones. The distinction matters — a URDF whose meshes
> are absent still *simulates* correctly (mass, COM and inertia come from the
> authored `<inertial>` blocks, and only the collider falls back to a
> placeholder), but it **renders nothing**, so a document imported from one
> shows an empty viewport in the app.
>
> `examples/robot-arm-2dof.urdf` is genuinely primitives-only, and
> `examples/floating-arm.vcad` is that robot imported with a synthesized
> floating base — the sample to reach for when you need geometry that resolves.

The hand-authored versions ship with primitive box / cylinder / sphere
geometry and are ready to simulate with no external dependencies.

The official versions are the upstream URDFs from Unitree's
`unitree_ros` repository, vendored verbatim. The G1 references ~20 MB
of STL meshes that are *not* checked in (see below to enable them).
The Go2 official URDF references DAE (Collada) meshes — vcad has an
STL loader but no DAE loader yet, so even with `--package-root` set
the meshes resolve to 1cm placeholder cubes today. Joint topology and
authored `<inertial>` properties still flow through correctly, so
physics behaves like the real robot to first order.

## Running a simulation

```bash
# Self-contained (works out of the box)
vcad simulate examples/unitree-g1.urdf --steps 240
vcad simulate examples/unitree-go2.urdf --steps 240

# Official URDFs without meshes — fall back to 1cm placeholder cubes per
# link, but joint topology and authored inertials are exact.
vcad simulate examples/unitree-g1-official.urdf --steps 240
vcad simulate examples/unitree-go2-official.urdf --steps 240
```

## Enabling meshes for the official URDF

Clone Unitree's full repo somewhere and run `vcad simulate` against the
URDF in its native location — the mesh paths inside the URDF are
relative (`meshes/pelvis.STL`, etc.), so vcad resolves them against the
URDF's parent directory:

```bash
git clone --depth 1 https://github.com/unitreerobotics/unitree_ros.git ~/unitree_ros
vcad simulate ~/unitree_ros/robots/g1_description/g1_23dof.urdf --steps 240
```

For URDFs that use `package://` URIs (common in ROS workspaces), pass
one or more `--package-root` flags pointing at directories that contain
the package roots:

```bash
vcad simulate /path/to/some_ros_robot.urdf \
  --package-root ~/unitree_ros/robots \
  --package-root ~/other_ros_workspace/src
```

## Why two G1 URDFs?

`unitree-g1.urdf` was authored before vcad had STL import; it captures
the same 23-DOF kinematic structure with pure primitive geometry, so it
exists independently of any external mesh files. `unitree-g1-official.urdf`
is the real Unitree descriptor, included now that vcad's URDF reader can
resolve it. Both are kept around so demos work without any setup, while
serious users can drop in upstream meshes when they want them.
