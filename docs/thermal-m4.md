# Heat-conduction FEA M4: `vcad.thermal-claims/1`

Fifth rung: the solver's outputs become receipt claims.

`receipt::predicted_claims` emits:

- **`t_max_c`** — hottest solid voxel, with its location in the note.
- **`theta_ja_c_per_w`** — per source ((T_src,max − T_ref)/P; suffixed
  `:<source>` when several sources exist). A zero-power source gets an
  explicit `theta_ja_undefined` claim stating that θ = ΔT/P has no value
  at P = 0 — stated, never NaN, never silently dropped.
- **`energy_balance_residual`** — the conscience, promoted to a claim: the
  note says outright that this closes to solver tolerance or the solution
  is wrong.

**Every temperature claim's note carries the missing physics**: conduction
only, the exact h values the prediction is priced at ("supplied, not
derived"), and the unmodeled radiation (~6 W/m²K equivalent). This is
enforced by test — a claim without its caveat fails CI, because a claim
that hides its h is a guess wearing a lab coat.

Provenance (`SolverProvenance`): grid + voxel pitch, CG tolerance /
iterations / final residual, the **entire boundary-condition set** as
human-readable strings, anisotropy state (`isotropic`/`diagonal`), and
whether geometry came through the voxelized-part seam. Basis is
`"predicted"` throughout; binding to measurements is M6.

Flagged follow-up (cross-crate, not this branch): register the family in
`crates/vcad-receipt` and expose a `predict_thermal` MCP tool — touches
`ir:gen` (two-crate export, names must stay unique) and the tool-surface
fixtures, same shape as the particle crate's flagged receipt PR.
