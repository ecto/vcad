# Motor v3 — Verification Ledger (post-fix toolchain)

Third pass, 2026-07-08, after PRs #470–#473 merged. Run by a single scripted pipeline
(`v3_run.py` against the local MCP build); one manual repair batch.

## Fix regressions (Phase A)

| Fix | Result |
|---|---|
| #470 watertight flange-with-holes | **PASS** — rotor iron `verify_spec` 9/9 (volume 6800–6950 window, watertight, bbox) — was red in v2 |
| #471 apply_edits @N refs + root consumption | **PASS** — probe: 1 part, exact volume; full assembly: **11 parts / 68,027 mm³** (v2: 68 parts / 307k) |
| #472 sheet-metal cost alignment | **PASS** — quote $20.96 vs laser $17.64 (×1.19, was ×3.0) |
| #473 DFM tie-aware min_clearance | **PARTIAL** — positions now reported, but 4 star-junction contacts still flagged despite being provably inside the tie region (dist ≤1.09mm vs radius 2.95mm). Follow-up chip filed with exact coordinates. Waived: net-aware DRC = 0 on the same board. |

## Board (`stator-v3.vcad`)

- Realizer + 2 feed stitches (same PHB/PHC island repair as v2, scripted this time).
- **DRC 0**, receipt `31161edd73a2ca11` (now includes unified DesignReceipt claims).
- Gerbers `fab/stator-v3-gerbers/`, JLCPCB $9.55/ea ×5.
- Note: `@N` symbolic refs in apply_edits are batch-local by design — split batches must
  renumber (my v3 script bug, not a tool bug).

## Assembly (`motor-assembly-v3.vcad`)

Clean build via `solid_from_board` + two `@N` batches. All six clearance assertions PASS
(air-gap 1.000mm, magnet-vs-heads 3.15, shaft-vs-stator 0.995, hub-vs-heads 5.05,
rotor-vs-board 4.00, shaft-vs-bearing 0.0498).

## BOM (`fab/BOM-v3.md` / `.csv`)

15 lines, all manufactured lines quote-linked with the aligned cost model.
**Grand total $269.32 landed** (incl. optional $42 induction-demo boards + spares).

## Scorecard across rounds

| | v1 | v2 | v3 |
|---|---|---|---|
| Winding interconnect | hand JSON surgery | realizer + 2 stitches | same, scripted |
| EM verdicts | bash model | 2 tool calls | scripted |
| Clearances | hand math + pairwise booleans | 6 named assertions | scripted, re-run green |
| Assembly integrity | phantom-volume kernel bug | ghost roots (68 parts) | **11 parts, exact volume** |
| BOM | hand-written markdown | bom tools, $271.94 | bom tools, **$269.32** |
| New bugs found | 3 | 4 | 1 (tie-exemption follow-up) |
