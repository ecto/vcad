# Variant parameter tables

A design family is one design at several sizes. The rana actuator family —
rana-100 → 100b → 100c → 60c print mule — was four copies of one generator
set, and the copies drifted: a fix found on the mule had to be re-derived by
hand for the full-size part, and the split between *what scales* (radii,
heights) and *what must not* (gear module, M3 hardware, minimum wall) lived in
prose in a design doc, where a reviewer had to catch it when it went wrong.

Variant parameter tables put that split in the parameter itself.

## The forms

A parameter-table document (conventionally `params.loon` or
`<family>.params.loon`) declares one or more base tables and any number of
variants that overlay them. The vocabulary extends the `defparam` form
parametric loon already uses, so a table row reads the same as a declaration
inside a model:

```loon
[deftable rana
  [defparam envelope_d 100.0 :unit "mm"
    :description "outer diameter of the actuator can"]
  [defparam shell_wall 2.4 :unit "mm"]
  [defparam rotor_r "envelope_d / 2 - shell_wall"]
  [defparam pocket_clearance 0.2 :unit "mm"]
  [defparam pocket_w "magnet_block_w + 2 * pocket_clearance"]

  [defparam gear_module 0.5 :unit "mm" :scale_with_envelope false
    :description "m0.5 is the PLA tooth floor — smaller teeth do not print"]
  [defparam min_wall 0.4 :unit "mm" :scale_with_envelope false]
  [defparam bolt_d 3.0 :unit "mm" :scale_with_envelope false
    :description "M3 — COTS hardware does not scale"]
  [defparam magnet_block_w 10.0 :unit "mm" :scale_with_envelope false]]

[defvariant rana_60c :from rana :scale 0.6
  :description "0.6x print mule"
  [override pocket_clearance 0.4
    :why "60c mule first print: 0.2 pocket was interference-tight, +0.2 fits"]]
```

- `[deftable <name> [defparam ...] ...]` — a named base table. A row's value is
  a literal or a formula string over other rows, using loon's existing
  expression language. Options: `:unit`, `:description`,
  `:scale_with_envelope`.
- `[defvariant <name> :from <parent> :scale <f> :description "..." <overlays>]`
  — a variant. The parent is a table or another variant, so families chain.
- `[override <name> <value> :why "..."]` — set a value outright. `:why` is
  carried into `diff` output, which is where a finding gets read back.
- `[scale <name> <factor> :why "..."]` — scale one parameter explicitly.

## Three rules

1. **Scale applies to literals, not formulas.** A derived parameter recomputes
   from its (already scaled) inputs, so it is never scaled twice. `rotor_r`
   above becomes `30 - 1.44`, not `47.6 × 0.6`.
2. **`:scale_with_envelope false` holds a value through a scale.** m0.5 stays
   m0.5 at 0.6×; M3 stays M3; a 10 mm COTS magnet block stays 10 mm. Asking for
   such a parameter to be scaled *directly* is an error naming the flag, not a
   silent shrink:

   ```
   $ vcad params resolve rana_60c_bad
   error: variant 'rana_60c_bad' scales 'gear_module' by 0.6, but 'gear_module'
   is declared :scale_with_envelope false in table 'rana' (m0.5 is the PLA tooth
   floor — smaller teeth do not print). A value in that class is set by
   something other than the envelope — override it outright with
   [override gear_module <value> :why "..."] if it really must change, or drop
   the flag if it really does scale.
   ```

   Flagging a *formula* is likewise rejected: a derived value follows its
   inputs, so the flag belongs on the inputs.
3. **A variant's scale applies before its own overrides.** An override is a
   measurement taken at the variant's own size — the mule's `pocket_clearance
   0.4` is 0.4 mm on the printed mule, not `0.2 × 0.6`. It lands on the scaled
   table verbatim.

## Worked example

```
$ vcad params resolve rana_60c --file rana.params.loon --table
rana_60c (rana → rana_60c), effective scale 0.6
  envelope_d              60mm  envelope scale in 'rana_60c': 100 × 0.6
  shell_wall            1.44mm  envelope scale in 'rana_60c': 2.4 × 0.6
  rotor_r                28.56  derived: envelope_d / 2 - shell_wall
  pocket_clearance       0.4mm  own override in 'rana_60c' (60c mule first
                                print: 0.2 pocket was interference-tight, +0.2 fits)
  pocket_w                10.8  derived: magnet_block_w + 2 * pocket_clearance
  gear_module            0.5mm  held at the 'rana' base value —
                                scale_with_envelope: false, so the 0.6× envelope
                                scale does not apply
  bolt_d                   3mm  held at the 'rana' base value — …
  magnet_block_w          10mm  held at the 'rana' base value — …
```

Without `--table`, `resolve` writes the flat table as JSON on stdout — one
entry per parameter with `value`, `unit`, `scale_with_envelope`, and a tagged
`source` (`base` / `held` / `override` / `scale-derived` / `derived`):

```json
{
  "name": "gear_module",
  "value": 0.5,
  "unit": "mm",
  "scale_with_envelope": false,
  "source": {
    "kind": "held",
    "table": "rana",
    "skipped_factor": 0.6,
    "flag": "scale_with_envelope"
  }
}
```

`diff` answers the promotion question — *what exactly does this mule finding
change?*

```
$ vcad params diff rana rana_60c --file rana.params.loon
rana → rana_60c
  envelope_d: 100 → 60
      envelope scale in 'rana_60c': 100 × 0.6 — was base table 'rana'
  shell_wall: 2.4 → 1.44
      envelope scale in 'rana_60c': 2.4 × 0.6 — was base table 'rana'
  rotor_r: 47.6 → 28.56
      derived: envelope_d / 2 - shell_wall — same rule, different inputs
  pocket_clearance: 0.2 → 0.4
      own override in 'rana_60c' (60c mule first print: 0.2 pocket was
      interference-tight, +0.2 fits) — was base table 'rana'
  pocket_w: 10.4 → 10.8
      derived: magnet_block_w + 2 * pocket_clearance — same rule, different inputs
```

Every held parameter is absent from the diff, which is the point: the m0.5
module, the M3 bolt and the COTS magnet block are identical at both sizes, and
the single line an author has to decide about when promoting the finding is
`pocket_clearance`, marked `own override` with the reason it was made.

`--json` emits the same as a machine-readable structure; `--exit-code` makes a
non-empty diff exit 1, for a CI gate that asserts two variants agree.

## Commands

```
vcad params resolve <variant> [--file <path>] [--table]
vcad params diff <a> <b> [--file <path>] [--json] [--exit-code]
vcad params list [--file <path>]
```

`--file` defaults to `$VCAD_PARAMS`, else `params.loon` in the working
directory, so inside a project the commands read as `vcad params diff 100c 60c`.

## Where it lives

`crates/vcad-loon/src/variants.rs` — parsing, resolution and diff; the
resolution step hands the overlaid table to `vcad_ir::resolve_parameters`, the
same evaluator document parameters use, so formulas mean exactly what they mean
elsewhere. Tests: `crates/vcad-loon/tests/variants.rs` against the rana fixture
in `crates/vcad-loon/tests/fixtures/rana.params.loon`.
