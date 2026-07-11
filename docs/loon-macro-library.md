# The loon macro library

Agents define reusable parametric macros (`define_loon`), instantiate them
(`call_loon`), and compose them inside any program (`create_cad_loon` +
`use_loons` / inline `loons`). A macro is plain loon source —
`[let <name> [fn [params…] …]]` — prepended to programs exactly like the
stdlib.

## Storage tiers

| Tier | Mechanism | Survives |
|---|---|---|
| Warm | in-process registry | instance lifetime |
| Local | JSON files under `VCAD_MCP_STATE_DIR/loon-macros` | restarts (stdio/local) |
| Hosted | `mcp_macros` table (migration 036), per-user via `MacroStore` | cold starts, cross-instance |
| Stateless | pass-by-value `macro`/`loons` args | everything (no server state) |

Hosted tier requires `SUPABASE_URL` + `SUPABASE_SERVICE_ROLE_KEY` and a
signed-in caller; `user_id` is always the verified token subject, never
tool input. Reads hydrate-on-miss (artifact-store pattern); writes are
best-effort and never fail the define. **Migration 036 is written but not
deployed** — run `supabase db push --dry-run` then `supabase db push`.

## The trust ladder

1. **Smoke-tested** (shipped): `define_loon` refuses source that doesn't
   compile or whose example call yields no geometry.
2. **Certified** (next rung, `certify_loon` — designed, not built): run
   verify-tier oracles over the macro's parameter range and store a
   `DesignReceipt` with the macro (the `receipt` column in migration 036
   reserves the slot). Sketch:
   - Sample the parameter box (corners + centroid, or user-declared ranges
     on each param).
   - For each sample: instantiate, then run declared claims —
     `predict_physics` at `fidelity=verify` (structural limits),
     `verify_spec` (geometric spec), `inspect_cad` bounds (mass/volume).
   - All samples pass on `basis=verified` → receipt stored, macro shows
     `certified: true` in `list_loons`; any fail/unverifiable → fail-closed,
     no badge.
   - A certified macro's receipt composes: a document built from certified
     macros can cite their receipts as claims with `subject:
     macro:<name>@<version>` — re-verified (Holds/Stale/Violated) when the
     kernel version changes.
