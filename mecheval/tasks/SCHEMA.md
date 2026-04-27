# Task schema

One JSON file per task. Filename must match the `id` field. Files in this directory are the **public** corpus — fair game for tuning. Held-out tasks live under `mecheval/private/` (gitignored).

## Shape

```json
{
  "id": "a1-plate-01",
  "suite": "A",
  "tier": "A1",
  "title": "Drilled plate",
  "prompt": "Make a 50mm × 30mm × 10mm plate centered on the origin...",
  "inputs": [],
  "checks": [ ... ],
  "anti_cheese": { ... },
  "limits": { ... },
  "pass_k": 5,
  "tags": ["primitives", "holes", "boolean"]
}
```

### Required fields

| Field | Type | Notes |
|---|---|---|
| `id` | string | Unique. Matches filename. Convention: `<tier>-<short-name>-<NN>`. Lowercase, hyphenated. |
| `suite` | `"A" \| "B" \| "C"` | Which suite. |
| `tier` | string | `A1`–`A6`, `B-boolean` / `B-step` / `B-fillet` / `B-solver` / `B-tessellate` / `B-dynamics`, or `C-reacher` / `C-picker` / etc. |
| `title` | string | Short human label. |
| `prompt` | string | The natural-language spec given to the agent. |
| `checks` | array | One or more grader checks (see below). All must pass for the task to score. |

### Optional fields

| Field | Default | Notes |
|---|---|---|
| `inputs` | `[]` | Optional starter files (paths relative to the task file). E.g. a starter `.vcad` for refactor tasks, a reference image for image-conditioned tasks. |
| `anti_cheese` | `{}` | Per-task structural constraints baked into the spec — see below. |
| `limits` | `{}` | Hard ceilings: `max_tokens`, `max_wallclock_sec`, `max_tool_calls`. |
| `pass_k` | `5` | The harness runs the prompt this many times; pass^k is the fraction of runs that meet *all* checks. |
| `tags` | `[]` | Free-form tags for filtering. |

## Check types

Every check is something the vcad kernel (or the vcad gym, for Suite C) can compute deterministically. No similarity scores, no LLM judgments.

### Suite A & B checks

```jsonc
// The output is a valid closed manifold solid.
{ "type": "valid_solid" }

// Bounding box (kernel-space, mm).
{ "type": "bbox",
  "min": [-25, -15, 0], "max": [25, 15, 10],
  "tolerance_mm": 0.1 }

// Volume / surface area / center of mass.
{ "type": "mass_props",
  "volume_mm3": 14717.3,                  // optional
  "surface_area_mm2": 4310.0,              // optional
  "center_of_mass": [0, 0, 5],             // optional
  "tolerance_pct": 0.5 }

// N cylindrical through-features (holes) of a given diameter.
{ "type": "hole_count",
  "diameter_mm": 3.0, "expected": 4,
  "diameter_tolerance_mm": 0.05 }

// Specific hole positions.
{ "type": "hole_positions",
  "diameter_mm": 3.0,
  "positions": [[20, 10, 0], [-20, 10, 0], [-20, -10, 0], [20, -10, 0]],
  "tolerance_mm": 0.1 }

// Detected fillet/chamfer with a target radius on a named edge class.
{ "type": "fillet_radius",
  "edge_class": "inside_corner",
  "radius_mm": 4.0, "tolerance_mm": 0.05 }

// Round-trip: export to STEP, re-import, mass-props match within tolerance.
{ "type": "step_roundtrip", "tolerance_pct": 0.1 }

// DRC / ERC clean (ECAD tasks).
{ "type": "drc_clean" }
{ "type": "erc_clean" }

// DFM rule check.
{ "type": "dfm",
  "rules": ["min_wall_1.5mm", "draft_1deg", "no_undercut"] }

// Refactor tasks: untouched parts have unchanged mass-props.
{ "type": "refactor_invariant",
  "untouched_parts": ["base_plate", "mounting_bracket"],
  "tolerance_pct": 0.01 }
```

### Suite C (Mech) checks

```jsonc
// Body assembly is valid (closed solids, joints connect, no inter-penetration at rest).
{ "type": "body_valid" }

// Forward kinematics reaches a workspace point under joint limits.
{ "type": "fk_reaches",
  "target": [0.20, 0, 0.10], "tolerance_m": 0.005 }

// Joint torque budget covers gravity + payload.
{ "type": "torque_budget",
  "payload_kg": 0.1, "safety_factor": 1.5 }

// Center-of-mass stays inside the support polygon during a rollout.
{ "type": "stable_during_rollout",
  "rollout": "default", "min_margin_mm": 5 }

// The gym rollout completes the goal.
{ "type": "task_success",
  "task": "reach_target",
  "params": { "target": [0.20, 0, 0.10], "tolerance_m": 0.005, "max_steps": 1000 } }
```

## Anti-cheese constraints

Per-task structural rules. Designs that violate these fail the task regardless of other checks.

```jsonc
{
  "min_rigid_bodies": 4,
  "min_actuated_joints": 2,
  "max_solid_count": 1,
  "max_total_mass_kg": 2.0,
  "joint_torque_ceiling_nm": 5.0,
  "required_links": ["foot_left", "foot_right", "end_effector"]
}
```

## Conventions

- **Coordinate system:** Z-up, mm for Suites A/B, meters for Suite C (matches the URDF / physics convention).
- **Tolerances:** explicit on every check. No implicit defaults — if a check has no tolerance, it's exact.
- **Determinism:** every check must be runnable headless from a `.vcad` file alone. No human judgment, no LLM judgment.
- **Versioning:** schema changes get a `schema_version` field added to the task file. We don't have one yet because we're at v0.
