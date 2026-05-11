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
| `suite` | `"A" \| "B" \| "C" \| "F"` | Which suite. |
| `tier` | string | `A1`–`A6`, `B-boolean` / `B-step` / `B-fillet` / `B-solver` / `B-tessellate` / `B-dynamics`, `C-reacher` / `C-picker` / etc., or `F1`–`F4` (Fit suite). |
| `title` | string | Short human label. |
| `prompt` | string | The natural-language spec given to the agent. |
| `checks` | array | One or more grader checks (see below). All must pass for the task to score. |

### Optional fields

| Field | Default | Notes |
|---|---|---|
| `inputs` | `[]` | Optional starter files. Either bare paths (legacy) or structured objects (see "Structured inputs" below) for image-conditioned and Fit-suite tasks. |
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
//
// tolerance_pct applies to:
//  - volume_mm3, surface_area_mm2: relative — actual within ±tolerance_pct%
//    of spec (e.g. 0.5 ⇒ ±0.5%).
//  - center_of_mass: tolerance_pct of the actual solid's bbox diagonal,
//    applied per component as absolute mm, with a 0.01mm floor so COMs
//    near origin still admit kernel rounding.
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

### Suite F (Fit) checks

Fit tasks evaluate accessory parts that mate with a *host* object. The host's
`.vcad` is provided as a non-agent-visible input (see "Structured inputs"
below) and is used only by the grader. The agent's output is the accessory
alone; the grader places host and accessory together in the declared frame
before evaluating these checks.

```jsonc
// Minimum separation between accessory and host across the assembly.
// `min_mm = 0` is allowed (touching). For snap fits use a small negative
// `min_mm` and a positive `max_mm3` interference budget (see below).
{ "type": "mate_clearance",
  "host": "host_geometry",        // refers to an inputs[] entry by `kind`
  "min_mm": 0.1, "max_mm": 0.5 }

// Volume of accessory ∩ host. Should be ~0 for slip fits, small for press.
{ "type": "interference_volume",
  "host": "host_geometry",
  "max_mm3": 1.0 }

// Surface area of accessory within `epsilon_mm` of the host. Proxy for
// retention / load transfer.
{ "type": "contact_area",
  "host": "host_geometry",
  "epsilon_mm": 0.2,
  "min_mm2": 200 }

// Overall accessory bounding-box ceiling (mm). Use to cap envelope cheese.
{ "type": "envelope",
  "max_mm": [80, 80, 40] }

// Physics: assemble host+accessory, apply gravity in stated direction,
// step `duration_sec`, measure relative drift between host and accessory.
// Backed by phyz; deterministic only on the canonical mecheval CI runner.
{ "type": "gravity_hold",
  "host": "host_geometry",
  "host_mass_kg": 0.5,
  "gravity_dir": [0, 0, -1],
  "duration_sec": 10,
  "max_drift_mm": 1.0 }

// Physics: pull the accessory away with `force_n` newtons along
// `direction`; pass = drift stays under `max_drift_mm` for `duration_sec`.
{ "type": "pull_force",
  "host": "host_geometry",
  "force_n": 5.0,
  "direction": [0, 0, 1],
  "duration_sec": 2,
  "max_drift_mm": 0.5 }

// Geometric form-lock: translate the accessory by intermediate
// fractions of `displacement_mm` along `direction` and compute peak
// interference with the host. Pass if the peak exceeds the as-designed
// baseline by at least `min_interference_gain_mm3`. Deterministic
// stand-in for `pull_force` for snap-fits / form-locked retention.
{ "type": "pull_retention_geometric",
  "host": "host_geometry",
  "direction": [0, 0, 1],
  "displacement_mm": 3.0,
  "min_interference_gain_mm3": 20.0 }
```

## Structured inputs

Image-conditioned tasks (Suite A vision tier) and the entire Fit suite use
structured `inputs` entries instead of bare paths. Each entry has a `kind`
and an `agent_visible` flag. Bare strings are still accepted for legacy
starter-`.vcad` cases.

```jsonc
"inputs": [
  // Photographs or renders shown to the agent. Always agent_visible.
  { "kind": "reference_image",
    "path": "assets/spacer-shaft-front.jpg",
    "agent_visible": true,
    "view": "front",                 // "front" | "side" | "top" | "hero"
    "image_kind": "photo",           // "photo" | "render"
    "scale_fiducial": {
      "type": "aruco_4x4_50mm",
      "marker_id": 0
    }
  },

  // Host geometry consumed by the grader. NEVER shown to the agent.
  // The harness enforces `agent_visible: false` — entries with this flag
  // are stripped from `prompt.attachments` before the solver is invoked.
  { "kind": "host_geometry",
    "path": "assets/spacer-shaft-host.vcad",
    "agent_visible": false,
    "frame": {
      "origin": [0, 0, 0],           // host placed here in the assembly
      "axis":   [0, 0, 1]            // declared "up/main" axis (mm)
    }
  },

  // Free-form numeric facts the agent is allowed to use. Always
  // agent_visible. Use sparingly — the point of vision tasks is for the
  // model to recover dimensions itself.
  { "kind": "known_dimensions",
    "agent_visible": true,
    "text": "The middle waist of the shaft is 20mm in diameter." }
]
```

`kind` values used by graders (`host_geometry`, etc.) are referenced from
check specs by name, not by path — the grader resolves the entry from
`inputs`. This keeps task files refactor-safe.

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

### Fit-suite anti-cheese

Additional fields specific to Suite F. Designs that violate these fail
regardless of other checks.

```jsonc
{
  // Accessory bbox ∩ host bbox, divided by host bbox volume. Stops the
  // "wrap the whole host" cheese (perfect contact, perfect interference).
  "max_overlap_with_host_envelope_pct": 30.0,

  // Floor on accessory volume — stops degenerate near-zero solids that
  // pass clearance trivially.
  "min_accessory_volume_mm3": 100.0,

  // Whether the accessory bbox must contain the host centroid. Tasks
  // shaped like "stand"/"cradle"/"base" want true; "cap"/"clip" want false.
  "must_enclose_host_centroid": false
}
```

## Conventions

- **Coordinate system:** Z-up, mm for Suites A/B, meters for Suite C (matches the URDF / physics convention).
- **Tolerances:** explicit on every check. No implicit defaults — if a check has no tolerance, it's exact.
- **Determinism:** every check must be runnable headless from a `.vcad` file alone. No human judgment, no LLM judgment.
- **Versioning:** schema changes get a `schema_version` field added to the task file. We don't have one yet because we're at v0.
