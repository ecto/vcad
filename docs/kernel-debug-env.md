# Kernel debug and escape environment variables

The kernel's hard bugs are found by turning one stage's output into a stream
you can read, or by switching one guard off and watching what changes. Those
switches were scattered across doc comments and commit messages; this is the
list.

None of them change what the kernel does by default. `VCAD_*_DEBUG` /
`*_TRACE` only print; `VCAD_NO_*` disable a guard or an optimisation and are
for bisecting, never for shipping.

## Booleans

| variable | what it does |
|---|---|
| `VCAD_UNION_TRACE=1` | One stderr line per pairwise union in an evaluated chain: fidelity, time, operand and result volumes. The first tool to reach for on a part that solves slowly or wrongly. |
| `VCAD_NO_UNION_TREE=1` | Evaluate the authored fold as written, instead of letting `vcad-eval` reassociate a stalled union chain. **Also the answer to a confusing bisection**: the reassociation keys off the length of the REMAINING tail, so a pipe truncated after N stages can take a different path than the same prefix inside the full part. Truncating the rana-60 stator after 14–21 of its 57 stages drops those solves to `TriangleSoup` while the whole part stays `Analytic`; set this to compare like with like. |
| `VCAD_NO_UNION_HOIST=1` | Disable hoisting of union operands out of the fold. |
| `VCAD_NO_UNION_REFEREE=1` | Stop the mesh boolean from refereeing a cracked analytic union, so the analytic result is returned whatever it looks like. |
| `VCAD_SPLIT_DEBUG=1` | Per-face splitter decisions — which curve cut which face, and why a cut was declined. Needs `--features debug-boolean`; the output is large, so wrap the operation of interest in markers rather than counting occurrences. |
| `VCAD_NO_ARCGUARD=1` | Let a circle split a planar face even when the arc runs along the face's own boundary. |
| `VCAD_TANGENCY_SNAP=1` | Opt-in: pin a circle split's crossing to the analytic touch point where the cutting circle is tangent to a boundary arc. Off by default because it costs the rana-60 stator 3.4× (29.8 s → 104.7 s) for four unpaired edges — and the cost is the snapping's effect on the splits, not finding the tangency. See `docs/boolean-multilump-union-diagnosis.md`. |
| `VCAD_BURIED_FACE_CHECK=1` | Opt-in: after every union/difference, check that no retained face has material on both sides — a missing trim, which no volume bound or edge count can see. Sound (zero false positives across the boolean suite) but with no demonstrated catch yet, and it costs a BVH per operand plus a handful of parity rays per face, so it is off. |
| `VCAD_NO_FREEZE=1` | Disable seam freezing. |
| `VCAD_NO_VBAND=1` / `VCAD_BAND_DEBUG=1` | Cylindrical band splitting: disable / trace. |
| `VCAD_NO_WELD=1`, `VCAD_NO_WELD2=1` | Disable the coarse seam-snap and boundary-vertex weld rounds in `repair::repair_topology`. |
| `VCAD_CLS_DEBUG=1` | Face classification probes and verdicts. |
| `VCAD_BOOLEAN_WARN=1` | Warn on degraded boolean results. |

## Mesh repair (export boundary)

| variable | what it does |
|---|---|
| `VCAD_REPAIR_TRACE=1` | One line per repair pass: triangle count, defective-edge count, and how far that pass moved the surface from what came in — plus a closing line with the net surface lost/added and the fraction of area past 0.02 mm and 0.1 mm. This is how the tear at the rana-60 stator's lead notch was found, and how `SHAPE_TOLERANCE` was chosen. |
| `VCAD_NO_SHAPE_GUARD=1` | Put every repair path back to `RepairPolicy::manifold_at_any_cost`, i.e. the behaviour before the shape guard: manifoldness pursued whatever it costs the part. For measuring a part both ways without rebuilding. See `docs/boolean-multilump-union-diagnosis.md`. |

## Caching and reproducibility

| variable | what it does |
|---|---|
| `VCAD_CACHE_DIR=<dir>` | Root-mesh cache location. **Use a fresh temp dir for any cache-sensitive test**: a warm cache changes behaviour, and cold-pass/warm-fail is a real failure mode — run twice against the same fresh dir to catch it. |
| `VCAD_CACHE=0` | Disable the root-mesh cache. |
| `VCAD_DETERMINISM_CHILD` | Set by the determinism harness in its own subprocess; not for hand use. |

## Test fixtures that live outside the repo

| variable | what it does |
|---|---|
| `VCAD_STATOR_LOON`, `VCAD_STATOR_OUTLINE` | The rana-60 stator's source and its 2D-CSG plan view, for the `#[ignore]`d `stator_measure` and `stator_section_gate` tests in `vcad-eval`. |
| `VCAD_FIDELITY_BLESS=1` | Rewrite the torture fidelity baseline instead of checking against it. |

## Related

- `docs/torture-track.md` — the adversarial corpus and its (platform-specific) baseline.
- `docs/boolean-multilump-union-diagnosis.md` — what the tracing above found on the stator.
