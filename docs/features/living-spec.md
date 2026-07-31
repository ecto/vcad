# Living Spec

**Score: 74/100** | **Priority: #16**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |

> Verified against repo 2026-07-30. The executable-validation core shipped through the MCP server rather than a `vcad test` CLI: labeled clearance assertions persist on the document (`check_clearance`) and re-verify as Holds/Stale/Violated via `build_receipt`/`verify_receipt` (fail-closed schema in `crates/vcad-receipt`), plus a `verify_spec` MCP tool. The YAML test schema and `vcad test` CLI/browser runners described below do not exist.

## Overview

The `.vcad` file IS the specification. It contains geometry, assembly, motion sequences, physics validation, and test cases — all in one runnable file. No separate PDFs, drawings, or spec documents.

A vcad document is not just a description of what something looks like. It's a complete, executable definition of what something *does* and how it must *behave*.

## Why It Matters

### The Traditional Workflow Problem

| Document | Purpose | Owner |
|----------|---------|-------|
| CAD file | Geometry | Designer |
| PDF spec | Requirements | Systems engineer |
| Test plan | Validation steps | QA |
| Validation report | Results | Test engineer |

Four documents. Four owners. Four chances to drift apart.

### Real Pain Points

- **Version hell**: "Which version is correct?" — always the question
- **Sync tax**: Engineers waste hours keeping docs in sync after every design change
- **Silent drift**: PDF says "5mm clearance" but CAD has 4.8mm — who notices?
- **Tribal knowledge**: "Oh, that tolerance is wrong in the spec, everyone knows to use the CAD"
- **Review theater**: Checking boxes without actually validating

### The Living Spec Solution

Single source of truth that EXECUTES eliminates ambiguity.

When the spec is the model, and the model is testable, "does it meet spec?" becomes a command you run — not a meeting you schedule.

## What's In a .vcad File

| Content | Description |
|---------|-------------|
| Geometry | Parametric DAG of operations — every feature traced back to intent |
| Assembly | Parts, instances, joints with full kinematic definitions |
| Motion | Keyframed sequences, trajectories, timed animations |
| Validation | Physics tests that must pass for the design to be "correct" |
| Constraints | Clearances, ranges, limits — the rules the design must obey |
| Metadata | Materials, tolerances, notes, revision history |

All stored as JSON. Human-readable. Diffable. Mergeable.

## Executable Tests

Tests live inside the document, right next to the geometry they validate:

```yaml
tests:
  - name: "Gripper reaches target"
    assert:
      - end_effector_position within 5mm of [300, 200, 100]

  - name: "No collisions in motion"
    assert:
      - collision_count == 0 during motion "pick-place"

  - name: "Joint limits respected"
    assert:
      - all joints within limits

  - name: "Minimum clearance maintained"
    assert:
      - clearance("housing", "pcb") >= 2mm

  - name: "Assembly fits in envelope"
    assert:
      - bounding_box within [500, 400, 300]
```

### Test Types

| Type | What It Checks |
|------|----------------|
| Static geometry | Dimensions, clearances, fits |
| Kinematic | Joint limits, reachability, range of motion |
| Dynamic | Collision detection during motion sequences |
| Physics | Forces, torques, stability |
| Parametric | Behavior across parameter ranges |

## User Experience

### Sharing

Share a `.vcad` file and the recipient can:
- **Run it** — not just view static geometry
- **Play motion sequences** — see how it moves
- **Execute tests** — verify it meets requirements
- **Modify parameters** — explore the design space

No "you need version X of software Y" — it runs in the browser.

### CI/CD Integration

```bash
# Validate design automatically
vcad test model.vcad

# Run specific test suite
vcad test model.vcad --suite "clearance-checks"

# Fail CI if any test fails
vcad test model.vcad --strict
```

Design validation becomes part of the build pipeline. Broken designs don't merge.

### Design Review

| Traditional | Living Spec |
|-------------|-------------|
| "Does this meet the 5mm clearance requirement?" | Run the clearance test |
| "Will this collide during operation?" | Play the motion, watch for red |
| "Is this within joint limits?" | Check the test results |
| Review meeting: 2 hours | Review command: 2 seconds |

### No More "The PDF Says X But The CAD Shows Y"

There is no PDF. There is no separate spec. The model IS the spec. If the model passes its tests, it meets spec. If you want different behavior, change the tests.

## Success Metrics

| Metric | Target |
|--------|--------|
| Single file contains 100% of design intent | No external spec documents required |
| Tests runnable via CLI | `vcad test` works offline |
| Tests runnable in browser | No software installation for reviewers |
| Export to legacy formats | PDF, STEP, drawings when needed for downstream |
| Test coverage | Every stated requirement has a corresponding test |

## Competitive Advantage

**No CAD file format is executable.**

- STEP files: Passive geometry dump
- Solidworks files: Geometry + feature tree (no tests)
- Onshape: Cloud-locked, no local execution
- Fusion 360: Proprietary, no CI integration

vcad files are programs that describe objects. They don't just say what something looks like — they define what it must do and prove that it does it.

## Implementation Status

| Component | Status |
|-----------|--------|
| Parametric geometry in .vcad | Complete |
| Assembly with joints | Complete |
| Motion sequences | Complete |
| Test definition schema | Partial — clearance assertions + receipt schema (`vcad-receipt`), not the YAML schema above |
| Test execution engine | Partial — `build_receipt`/`verify_receipt`/`verify_spec` MCP tools |
| CLI test runner | Planned |
| Browser test runner | Planned |

## Related Features

- [Physics Simulation](./physics-simulation.md) — Powers dynamic validation tests
- [Motion Sequences](./motion-sequences.md) — Defines trajectories to test
- [Assembly Joints](./assembly-joints.md) — Kinematic constraints to validate
