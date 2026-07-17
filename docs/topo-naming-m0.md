# Persistent topological naming — M0→M2 design note

The foundation for robust parametric editing: **stable names for BRep faces
and edges derived from generating operations**, so a downstream feature (a
fillet, a sketch-on-face) can survive an upstream parameter change instead of
breaking or silently attaching to the wrong entity — the classic FreeCAD
"topological naming problem". This note records the resolved ground truth,
the design decisions and their reasons, and what each milestone gates.

## What landed

- `vcad-kernel-naming` (new crate) — the naming scheme itself: `FaceName`
  (`scope:tag[.n]*`), the `NameMap` side table, geometry-derived seeding
  (`seed_names`), boolean propagation by surface identity
  (`propagate_boolean`), edge references by adjacent-face-name pair
  (`EdgeRef`), and fail-closed resolution (`resolve_edge` →
  `Resolved / Ambiguous / Lost`).
- `vcad-kernel` — `Solid` carries an `Option<NameMap>`: primitive
  constructors seed it, booleans propagate it, rigid transforms carry it,
  and everything without a propagation rule drops it (fail-closed). Public
  API: `names()`, `set_name_scope()`, `resolve_named_edge()`,
  `edge_blend_named()` (+ `NamedEdgeError`).
- `vcad-kernel-fillet` — `EdgeQuery::Endpoints`, an exact quantized
  endpoint-pair selector: the deterministic handoff from a resolved named
  edge into the existing blend pipeline (no fuzzy nearest-edge behavior).
- `vcad-ir` + evaluators — `EdgeQuery::Named { face_a, face_b }` on
  `CsgOp::EdgeBlend`, wired through all three Rust evaluators (`vcad-eval`,
  `vcad-app`, `vcad-kernel-wasm`) with the same node-id scope convention;
  unresolvable references are evaluation errors, never guesses.
- Regression tests at every layer: `vcad-kernel-naming` unit tests,
  `crates/vcad-kernel/tests/topo_naming.rs`, and the M2 gate in
  `vcad-eval` (`named_edge_blend_survives_box_resize`).

## Step 0 — resolved ground truth

Surveyed before designing; three facts shaped everything:

1. **No provenance exists anywhere.** The half-edge arena
   (`vcad-kernel-topo`) stores slotmap keys only — no name, label, or
   source-face field on any entity. Every pipeline stage (primitive →
   boolean → fillet) produces a **fresh `BRepSolid` with brand-new keys**.
   Greenfield: nothing to migrate, nothing to fight.
2. **Primitive face construction is deterministic**, and face roles are
   fully recoverable from geometry (a cube's six axis-aligned plane normals,
   a cylinder's lateral surface + caps). So seeding does not need to thread
   labels through the constructors at all — it can *derive* them.
3. **The boolean pipeline computes the result→input face map and throws it
   away** (`sew.rs::copy_faces` returns `HashMap<FaceId, FaceId>`; every
   call site binds it to `_`), and the boolean's enumeration order is
   **nondeterministic** (documented in `differentiable-seam-m0-m2.md`: cap
   faces can swap between builds at nearby parameters). Split sub-faces,
   however, always **inherit the parent face's carrying surface**.

Fact 3 drove the central design decision: **propagation is a geometric
post-pass, not a pipeline thread-through.** Matching each result face to the
input face with the geometrically identical carrying surface (kind +
analytic parameters within 1e-6 mm) recovers exactly the mapping the sewing
stage discards, without touching the 15 kLOC boolean crate — and, unlike
traversal order, it is deterministic by construction. The alternative
(threading provenance through split → classify → sew) remains available as a
later optimization; it would change *how* the same names are computed, not
*what* they are.

## The name scheme (M0)

`scope:tag[.ordinal]*`, e.g. `n3:top`, `n1:side`, `n3:top.0`.

- **scope** — the generating operation. Primitive constructors seed with
  the primitive kind (`cube`, `cylinder`, …); the DAG evaluators rescope to
  `n<nodeId>` immediately after evaluating a primitive node, because node
  ids persist in the `.vcad` document (rebuild-stable) and are unique across
  the DAG (two cubes in a union don't collide). All three evaluators use the
  same convention, so a `Named` reference means the same thing in the app,
  the CLI, and the browser.
- **tag** — the face's role, derived from its carrying surface:
  axis-aligned planes → `bottom/top/front/back/left/right` (Z-up), other
  planes → `plane`, cylinders → `side`, cones/spheres/tori → kind name.
  Repeated tags (a prism's walls) get ordinals in **quantized-centroid
  order** — deterministic regardless of arena iteration order.
- **path** — one sibling ordinal per boolean split generation. When a
  boolean cuts `cube:top` into two pieces, they become `cube:top.0` /
  `cube:top.1`, ordered by quantized centroid. A face that splits again in a
  later boolean appends another ordinal.

Edges are **not named independently**: an edge reference is the canonical
(sorted) pair of its adjacent faces' names. This eliminates an entire
propagation channel — edge identity rides on face identity.

Fail-closed rules in propagation: a result face whose surface matches
*no* named input face, or matches faces carrying *different* names (two
flush coplanar faces from both operands), stays anonymous. Anonymous faces
make downstream references report `Lost` — never a guess.

## Resolution + fallback (M1)

`resolve_edge(brep, names, EdgeRef) → EdgeResolution`:

1. **By name**: find the unique face carrying each name, then the unique
   manifold edge adjacent to both → `Resolved { method: ByName }`. This is
   the path that survives parameter edits, because names re-derive
   identically on rebuild.
2. **By geometry** (only when names fail and the reference carries an
   `EdgeHint` captured at reference time): candidates must agree with the
   recorded direction (within ~18°, sign-insensitive), length (±25 %), and
   midpoint (within 25 % of recorded length — all tolerances scale with the
   edge, so the matcher is scale-free). Exactly one candidate →
   `Resolved { method: ByGeometry }`.
3. Anything else is explicit: several candidates → `Ambiguous { candidates }`,
   none → `Lost { reason }`. Both are hard errors at every consumer — the
   blend is *not* applied, the evaluator reports which reference broke and
   why. A stale hint after a large edit is `Lost`, not rebound (gated by
   `stale_hint_after_a_large_edit_is_lost_not_rebound`).

Two faces sharing several edges (a through-slot) are `Ambiguous` by name
alone; the hint disambiguates when present.

## Consumer wired end-to-end (M2)

`vcad-ir` gains `EdgeQuery::Named { face_a, face_b }` on `CsgOp::EdgeBlend`
(serde-tagged, ts-rs exported; `npm run ir:gen` regenerated the TS types).
Evaluation resolves the names against the child solid's map and hands the
result to the fillet crate as `EdgeQuery::Endpoints` — an exact quantized
endpoint match, so the blend lands on precisely the resolved edge (the
pre-existing `Near` query has nearest-endpoint tie hazards that make it
unsuitable as a resolution target).

The M2 gate (`named_edge_blend_survives_box_resize`, in `vcad-eval`): a
document with `Cube(sx, 10, 10)` → `EdgeBlend(Named("n0:top", "n0:right"),
fillet r=2)` evaluates at `sx = 10` and `sx = 14`; at both sizes the blended
volume matches the closed form `sx·100 − (1 − π/4)·r²·10` to < 0.5 mm³, no
vertex remains on the intended corner line (x = sx, z = 10), and the
opposite edge stays sharp. The fail-closed side is gated too: a `Named`
reference to a nonexistent face is an `EvalError::NamedEdge`, and the
kernel-level `unresolvable_references_fail_closed` covers bad names, dropped
maps, and no-match `Endpoints` queries.

Boolean propagation is gated by
`boolean_propagates_names_deterministically` (kernel tests): cube −
through-cylinder keeps all six `cube:*` names, imports `cylinder:side` for
the hole wall, and two rebuilds at different radii produce the identical
name set.

## Design decisions and their reasons

- **Side table, not a topology field.** `BRepSolid` has ~96 construction
  sites across 20+ crates; a `name` field on `Face` would touch all of them
  and force every topology-rebuilding op to have an opinion. The
  `NameMap` lives on `vcad_kernel::Solid` (one crate, ~30 literal sites),
  and ops that can't propagate simply drop it — absence is an honest,
  fail-closed signal.
- **Names are derived, not stored provenance.** Seeding from geometry and
  propagating by surface identity makes naming a pure function of the
  operation graph — the property that makes names *rebuild-stable*, which
  stored arena-key lineage can never be (the boolean enumerates
  nondeterministically).
- **Determinism via quantized-centroid ordering** everywhere an ordinal is
  assigned (1e-6 mm grid). Centroids of distinct sibling faces are far
  apart relative to the quantum; hash-map iteration order never leaks into
  a name.
- **`Endpoints`, not `Near`, as the resolution handoff** — exact matching
  keeps the fail-closed guarantee through the last hop into the blend
  pipeline.

## Known limitations (deliberate, in scope-fence order)

- **Fillet/chamfer/shell/sweep outputs drop the map.** Blend faces need
  generated names (`<edge-ref>:blend`) and trimmed neighbors need
  carry-through — the same surface-identity post-pass would work (trimmed
  faces keep their planes) and is the natural M3.
- **Sketch-derived solids (extrude/revolve/loft) are unnamed.** Extrude
  wants `side loop L, segment S` names seeded from the profile — M3/M4
  territory, and the reason `FaceName.tag` is a free string rather than an
  enum.
- **Coplanar-merge ambiguity is resolved by anonymity.** A union of two
  boxes sharing a face plane names the merged face neither `a:top` nor
  `b:top`. Correct but lossy; a merge rule (`a:top+b:top`) could name it.
- **Same-name symmetry under exact symmetry.** Two split siblings that are
  perfectly symmetric about the centroid-ordering axes sort by the next
  coordinate; a parameter change that swaps their sorted order renames
  them (`.0` ↔ `.1`). The `EdgeHint` fallback catches the reference; a
  parameter-continuity heuristic (nearest-previous-centroid) would be the
  refinement.
- **`surface_tag` classification can jump.** A plane rotated onto an axis
  changes tag (`plane` → `top`). References through such an edit fall back
  to the hint. Tag-from-op (rather than tag-from-geometry) seeding would
  remove this; it costs threading labels through the primitive
  constructors.
- **App/WASM UI wiring is a follow-up.** The WASM evaluator resolves
  `Named` queries (browser documents evaluate correctly), but there is no
  UI for picking a named edge or displaying names yet, and no MCP tool
  exposes name introspection. Follow-up milestone: `list_names` on the
  inspect path + a picker that writes `Named` queries instead of `Near`.

## What M3 needs from this

Fillet-output propagation is the unblocking step: `apply_edge_blend`
rebuilds topology via the same face-extraction helpers everywhere, and the
surviving faces keep their carrying surfaces — so `propagate_boolean`'s
surface-identity matcher generalizes as-is (rename it `propagate_rebuild`).
With that, chained features (fillet-then-fillet, fillet-then-boolean)
resolve, and `sketch-on-named-face` becomes the first non-edge consumer:
`FaceRef` (a `FaceName` + planar fingerprint) resolving to an attachment
plane, with the identical fail-closed reporting.
