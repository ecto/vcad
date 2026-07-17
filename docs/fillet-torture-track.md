# Fillet torture track

An adversarial corpus of fillet/blend cases plus a never-regress scoreboard,
used to harden `crates/vcad-kernel-fillet`. Lives at
[`crates/vcad-eval/tests/fillet_torture.rs`](../crates/vcad-eval/tests/fillet_torture.rs)
with the loon corpus in
[`crates/vcad-eval/tests/fillet_torture/`](../crates/vcad-eval/tests/fillet_torture/).

Run it:

```bash
cargo test -p vcad-eval --test fillet_torture -- --nocapture
```

## Corpus

21 loon programs + 4 kernel-level `edge_blend` cases. Each `.loon` file
evaluates to a two-root scene — root 0 the filleted body, root 1 the same body
without the fillet — so the harness can tell the kernel's documented fail-soft
path (returning the input unchanged) apart from a real success.

| Group | Cases |
|---|---|
| Controls | `cube-baseline`, `refillet-larger` (parameter-change rebuild) |
| Boolean seams | `seam-union-cubes`, `seam-tee`, `boss-on-plate`, `notch-difference`, `hole-difference` |
| Radius vs feature size | `radius-near-half` (r=4.5 on a 10mm cube), `radius-half`, `radius-exceeds` (r=6), `thin-slab` |
| Non-orthogonal planar | `wedge-fillet`, `prism3-fillet`, `fillet-after-chamfer` |
| Curved primitives | `cylinder-cap`, `cone-fillet`, `slot-extrude` (tangent line-arc chains) |
| Curved-curved seams | `cyl-cross`, `sphere-box`, `lens-spheres`, `fillet-of-fillet` |
| Kernel `edge_blend` | variable radius along one edge, chamfer→fillet morph, direction-selected constant fillet, keyed profile over all edges (vertex-adjacent skip path) |

Outcomes are classified as `Success` (watertight, sane volume, distinct from
input), `NoOp` (fail-soft input-unchanged), `Refused` (clean error),
`BadGeometry` (open mesh edges, volume growth/collapse — never acceptable), or
`Crash` (never acceptable). The scoreboard asserts every case stays at or
above its recorded baseline rank, and that `Crash`/`BadGeometry` counts are
zero regardless of baseline.

## Baseline (before hardening)

First run: **9/25 success, 10 no-op, 6 silently-bad, 0 crash.**

The six silently-bad cases:

- `radius-exceeds` — infeasible radius produced a watertight but *inverted*
  shell that **gained** volume (1067 mm³ from a 1000 mm³ cube).
- `wedge-fillet`, `prism3-fillet` — 48–60 open mesh edges.
- `fillet-after-chamfer` — 1248 open edges.
- `slot-extrude`, `fillet-of-fillet` — 176/382 open edges via the curved path.

## Fixes

1. **Dihedral-correct trim setbacks** (`trim.rs`, `fillet_planar.rs`). The
   planar pipeline inset every face by a uniform `r`, which is only the
   correct tangent setback for 90° dihedrals; the true setback is
   `r/tan(θ/2)` (2.4× larger on a 45° wedge edge). `fillet_edge_setbacks`
   now computes per-edge setbacks; knife edges (setback > 8r) refuse cleanly.

2. **General tri-tangent corner spheres** (`trim.rs`). Corner blends only got
   true sphere octants at cube-style orthogonal vertices; everything else fell
   back to a flat triangle that cracked against the adjacent blend arcs.
   `try_sphere_blend` now solves the three-plane tangency system
   `nᵢ·(c−v) = −r` directly, so any well-conditioned 3-face corner gets an
   exact tangent sphere.

3. **Epsilon-tolerant arc subdivision counts** (`vcad-kernel-tessellate`).
   A 135° blend arc at 32 segments lands exactly on `ceil(12.0)`; float
   jitter made the cylinder side round to 13 while the sphere side rounded
   to 12, cracking the weld along the shared boundary arc. All three arc
   subdivision sites now use `(x − 1e-9).ceil()`.

4. **Feasibility guard** (`fillet_planar.rs`). After trimming, every inset
   face polygon must keep its original orientation and non-degenerate area;
   inverted insets (radius exceeding the local feature size) refuse instead
   of emitting a volume-gaining shell.

5. **Post-flight validity gate** (`vcad-kernel::blend_result_is_valid`).
   `Solid::fillet`/`chamfer`/`edge_blend` now validate the rebuilt shell.
   The planar pipeline must be perfectly watertight with shrinking (but not
   collapsing) volume. The curved per-edge pipeline intentionally tolerates
   residual corner-blend gaps (the arc-profile sphere-blend regression test
   depends on this), so it is gated on total open-edge length ≤ 2.0× the
   input bbox diagonal — shipped-good curved fillets measure ≈1.4×, broken
   rebuilds 2.6–26×.

## After hardening

**11/25 success, 14 no-op, 0 silently-bad, 0 crash.** Promoted to full
success: `wedge-fillet`, `prism3-fillet`, `fillet-after-chamfer`,
`thin-slab`; `radius-exceeds` and `radius-half` now refuse cleanly.

## Remaining known-failure classes (graceful no-ops)

- **Boolean-seam planar solids** (`seam-union-cubes`, `seam-tee`,
  `boss-on-plate`, `notch-difference`): boolean sewing leaves coplanar split
  faces (interior dihedral ≈ π trips the coplanar safety check) and concave
  edges, which the inset-all-faces planar pipeline cannot express. Needs
  coplanar-face merging plus concave (material-adding) blend support.
- **Plane–cylinder / plane–cone rim rebuild** (`cylinder-cap`, `cone-fillet`,
  `slot-extrude`, `fillet-of-fillet`): `fillet_edges_detailed` reports
  per-edge `Success` but the rebuilt shell around torus cap blends is cracked
  and volume-collapsed; the new gate converts this from silently-bad to a
  clean no-op. The rebuild itself still needs fixing.
- **Curved-curved intersection edges** (`cyl-cross`, `sphere-box`,
  `lens-spheres`): skipped by `collect_fillet_target_edges` (fragile
  rolling-ball) or blocked upstream by boolean inner loops
  (`hole-difference`).
- **Vertex-adjacent selections in keyed `edge_blend`** are skipped by design
  (miter corners are a planned follow-up); the harness verifies they never
  crash.
