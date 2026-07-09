# Edge blends: queries + keyed profiles

`EdgeBlend` is vcad's per-edge blend operation. It generalizes `Fillet` and
`Chamfer` along two orthogonal axes:

1. **Which edges** — a declarative `EdgeQuery` resolved against the child's
   topology at evaluation time, instead of "all edges" or a fragile index.
2. **What cross-section** — a `BlendProfile` that is either constant or
   keyframed along each edge, morphing continuously between a flat chamfer
   and a round fillet (the "chamfer into a fillet loft").

```jsonc
// A 3 mm chamfer at the top of a vertical edge melting into a fillet at
// the bottom — pick the edge by a point near its top corner:
{
  "type": "EdgeBlend",
  "child": 4,
  "edges": { "type": "Near", "point": { "x": 0, "y": 0, "z": 50 } },
  "profile": {
    "type": "Keyed",
    "keys": [
      { "t": 0.0, "size": 3.0, "shape": 0.0 },   // shape 0 = chamfer
      { "t": 1.0, "size": 3.0, "shape": 1.0 }    // shape 1 = fillet
    ]
  }
}

// A constant 2 mm fillet on every vertical edge:
{
  "type": "EdgeBlend",
  "child": 4,
  "edges": { "type": "Direction", "axis": { "x": 0, "y": 0, "z": 1 }, "tol_deg": 5 },
  "profile": { "type": "Constant", "size": 2.0, "shape": 1.0 }
}
```

## Why queries instead of edge ids

BRep edge ids are artifacts of one evaluation: change an upstream parameter
and the topology is rebuilt with different ids (and possibly a different
number of edges). A query is *intent* — "the edge near this corner", "all
vertical edges" — and re-resolves against whatever topology the child
produces today. This is the same property that makes the op usable by AI
agents over MCP: the selection language is declarative data, not click
coordinates in a session.

Current queries (`crates/vcad-ir` → `EdgeQuery`, mirrored in
`vcad-kernel-fillet`):

| Query | Meaning |
|---|---|
| `All` | every plane-plane manifold edge |
| `Near { point }` | the single edge whose nearest endpoint is closest to `point`; that endpoint becomes the profile's `t = 0` end |
| `Direction { axis, tol_deg }` | edges within `tol_deg` of `axis` (sign ignored) |

Resolution is deterministic (matches are ordered by quantized start
position) so re-evaluation is reproducible. Planned extensions: `OnFace`,
`BetweenSurfaces`, boolean combinators (`And`/`Or`/`Not`), and structured
resolution diagnostics ("matched 4 edges last time, 3 now") surfaced in the
feature tree.

## The profile model

A blend cross-section at any point along an edge is a curve between the two
tangency points on the adjacent faces, at tangent setback `size` (chamfer
leg = fillet radius). A chamfer is the straight line between the tangencies;
a fillet is the circular arc. Because both share endpoints, any convex
combination is a valid section: `shape` interpolates them (`0` = chamfer,
`1` = fillet).

`BlendProfile::Keyed` places `{ t, size, shape }` keyframes along the edge
(piecewise-linear between keys, clamped outside). One key ≡ `Constant`.
Key positions are inserted into the axial sample grid so profile kinks land
exactly on a sampled section. This one mechanism covers:

- classic fillet / chamfer (constant, shape 1 / 0)
- chamfer→fillet lofts (2 keys)
- tapered blends (size varies)
- multi-segment treatments (fillet–chamfer–fillet along one edge)
- future G2/conic sections: `shape > 1` is reserved for fuller-than-circular
  conic profiles — the section sampler is the only code that changes.

## Kernel construction (why it's watertight)

`crates/vcad-kernel-fillet/src/blend_loft.rs`, following the architecture of
`fillet_subset`:

- Every section ring is sampled once and reused verbatim by the blend strip,
  the cap-face splices, and the side-face insets; vertices weld through a
  quantized cache, so the rebuilt solid is watertight **by construction**
  rather than by stitching.
- Side faces stay planar for any size profile (tangency points always lie in
  the face plane); their loops carry every axial sample so the tessellation
  has no T-junctions.
- The strip is emitted as planar triangles over the section grid, wound
  outward against `n_a + n_b`.

Verified in tests against closed forms: constant chamfer to 1e-6 relative
volume, constant fillet, chamfer→fillet loft against a Simpson integral of
exact section areas (the area is quadratic in the shape parameter), plus
watertightness (`boundary_edges() == 0`) on every case including keyed
3-section profiles and multi-edge query application.

## Evaluation semantics

`Solid::edge_blend(query, keys)` in `vcad-kernel`:

- `All` + constant pure fillet/chamfer routes to the existing analytic
  whole-solid pipelines (cylindrical/toric blend surfaces, sphere corner
  patches) — no behavior change for classic fillets.
- Everything else resolves the query and blends edge-by-edge, rebuilding the
  solid each step and re-locating remaining edges by quantized endpoints.
  Edges sharing a vertex with an already-blended edge are **skipped** (the
  kernel-level `apply_edge_blend` reports `matched/blended/skipped` in a
  `BlendOutcome`): blending two edges into a shared corner needs the miter
  construction, which `fillet_subset` implements for constant radii and is
  the planned follow-up for variable profiles.
- Fail-soft like `fillet`: mesh-only or empty solids pass through unchanged.

## Serialization

- `.vcad` JSON: the op serializes with serde-tagged `EdgeQuery` /
  `BlendProfile` (TS types generated via `npm run ir:gen`).
- vcode: `EB <child> <edges-json> <profile-json>` — each payload is compact
  JSON carried as one quoted, escaped string token.
- loon: no syntax yet (emitted as an unsupported-op comment).

## Roadmap

1. **Miter corners for keyed profiles** — extend `fillet_subset`'s exact
   bisector-plane trim to variable sections so a blend can turn a corner;
   removes the `skipped` category for chains.
2. **Draft** — `Draft { faces: FaceQuery, neutral_plane, angle_deg }`, the
   same query pattern face-flavored; pairs with the DFM rule packs
   (undrafted-face findings can emit a ready-to-apply `Draft` node).
3. **Query extensions + diagnostics** — `OnFace`, combinators, and matched-
   count drift warnings surfaced as data for the feature tree and agents.
4. **UI** — click-an-edge direct manipulation: draggable end handles for
   size, a chamfer⇄fillet morph slider per key, drag-along-edge to add keys,
   and query-capture chips that generalize a multi-selection ("these 4
   edges" → "all vertical edges"). The keyed-profile IR is deliberately
   shaped so the UI writes the same vocabulary agents do.
