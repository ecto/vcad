# Sheet Metal: A First-Principles Spec

> The strategic vision and UI plan for making vcad the best sheet-metal CAD tool ever shipped.
> For technical reference (math, algorithms, gauge tables), see [`features/sheet-metal.md`](../features/sheet-metal.md).

## What sheet metal really is

A sheet-metal part is a 3D solid that admits an **isometric flattening**: there exists a developable
surface (zero Gaussian curvature everywhere) of uniform thickness `t` whose mid-surface unfolds onto
a planar region without stretching. Bends are cylindrical patches — the only surface a brake press
can actually make. Everything else (lofted flanges, conical transitions) is an approximation we owe
the user clarity about.

This means sheet metal is not "a feature" — it's a **constraint manifold inside the BRep state
space**. Every operation either preserves manufacturability or it doesn't. Existing tools treat
sheet metal as a parallel modeling mode with its own broken feature set; the legendary move is to
treat it as a manufacturability constraint that any operation can be checked against, with a kernel
that natively represents bends as first-class topology.

## Why incumbents fail (the problems worth solving)

1. **K-factor is a single global lie.** Real shops have bend deduction tables indexed by
   `(material, thickness, R/t ratio, V-die width, grain)`. SolidWorks/Onshape ship a number; shops
   keep the truth in spreadsheets and override every part.
2. **Flat pattern is a derived view, not a model.** You can't add tooling holes, registration tabs,
   or nest-friendly cutouts on the flat and have them survive. Manufacturing edits get re-done every
   revision.
3. **Bidirectionality is fake.** Unfold→edit→refold is not idempotent in any commercial kernel.
   Numerical drift accumulates.
4. **Corner relief is heuristic spaghetti.** Each tool has a dozen relief styles, none of which work
   cleanly at non-orthogonal corners or three-way intersections.
5. **No springback model.** Designers compensate by guessing.
6. **DFM is post-hoc.** Bends too near edges, holes too near bends, collisions with the brake's
   back-gauge — all caught by the shop, hours later.
7. **Lofted/transition flanges produce wrong flat patterns.** Ruled-surface approximation with no
   honest error metric.
8. **Tooling is invisible to the CAD.** It doesn't know your brake's max bend length, your die set,
   your minimum flange height for a given die.
9. **Costing is a separate $20k product.** Should be a derivative of the model.
10. **AI agents can't design sheet metal** because no public CAD exposes manufacturability as a
    queryable, differentiable interface.

## The legendary architecture

### 1. Bends as first-class topology

In `vcad-kernel-topo`, introduce a `BendRegion` annotation on a connected band of cylindrical faces
with metadata `{axis, radius, angle, k_factor_source, neutral_line}`. This isn't just a tag — it
changes how booleans, fillets, and tessellation behave near bends, and it's what makes lossless
unfold possible.

`BaseFlange`, `EdgeFlange`, `MiterFlange`, `Hem`, `Jog`, `SweepFlange`, `LoftedFlange` (with honest
developability error reported), `Tab`, `Louver`, `Lance`, `FormingTool` (user-defined dies stamped
from a sketch+depth profile) all emit BRep with `BendRegion`s annotated. No flange ever becomes
"just geometry."

### 2. Lossless bidirectional unfold

`Unfold` is an **involution**: `refold(unfold(x)) == x` to within a documented tolerance proven by
test. Achieve this by carrying the bend metadata through the flattening — the flat pattern stores
not just 2D loops but a `Crease` graph with `(line, angle, radius, K, direction)`. The 3D model and
flat pattern are **two views of the same IR node**, not derivations. Edits on either view emit IR
ops that the other view re-evaluates.

This is the single most differentiating feature. Nobody else has it because nobody else built it
from day one.

### 3. Bend tables, not K-factors

A `BendTable` is a queryable function
`(material, t, R, die_width, grain) → (BA, BD, K, springback_angle, min_flange, recommended_relief)`.
Ships with:

- A curated default table sourced from public Machinery's Handbook + DIN data.
- An **open, community contribution path**: shops submit measured tables under a permissive license;
  we publish a versioned, peer-reviewed registry. This is the "Wikipedia of bend allowances" — the
  moat is data, and the data wants to be free.
- A learning mode: if a shop measures actuals on test coupons, vcad fits a model and stores the
  residuals.

Every bend in a model carries a **provenance pointer** to the table row that produced its allowance.
Change the table, the model updates. This is real, not marketing.

### 4. Manufacturability as a typed query

Expose a single Rust API:

```rust
fn check_manufacturability(part: &Sheet, shop: &ShopProfile) -> Vec<Violation>
```

`ShopProfile` describes brakes (max length, tonnage, die library), lasers (max sheet, kerf),
materials in stock, grain rules. `Violation` is structured:
`BendTooCloseToEdge { bend_id, edge_id, actual_mm, required_mm }`,
`HoleInsideBendRelief { hole_id, bend_id }`,
`FlangeBelowMinHeight { ... }`,
`BendCollidesWithBackGauge { ... }`. Surfaced live in the property panel as squiggles, exposed via
MCP so AI agents only ever produce buildable parts.

### 5. Springback as physics, not a fudge

Use a closed-form elastoplastic beam-bending model (Marciniak / Hosford) for v-bends, parameterized
by yield strength, modulus, and strain-hardening exponent — all in the materials registry. The
kernel emits the **as-bent** geometry and the **target** geometry separately; either can be the
dimensioned reference. For exotic cases, plug `vcad-kernel-physics` (already in-tree) into a forming
sim mode.

### 6. Costing as a derivative of the model

`cost(part, shop) → {sheet_area, perimeter_cut, pierces, bends, setups, time, currency}` — a pure
function of the IR. Live in the UI. Shops calibrate with two real quotes and the model stays
accurate. This kills the "send to shop, wait three days for quote" loop.

### 7. Flat-pattern-first authoring

A power-user mode where you draw the **flat** with bend lines, set angles per crease, and the 3D
form materializes. This is how experienced sheet-metal designers actually think. Onshape kinda has
it, badly. We make it the equal partner of 3D-first authoring because the IR is the same either way.

### 8. Nesting and DXF/DWG that the shop accepts

Multi-part rectangular and true-shape nesting (`vcad-kernel-nest`) producing layered DXF: cuts on
one layer, bend up/down on others, etch text on another, with the **exact conventions** Trumpf /
Amada / Mazak post-processors expect. Round-trip tested against real machines via partnerships.
Optional G-code for routers and Gerber-style outputs for waterjets.

### 9. Welded sheet-metal assemblies

A `Weldment` IR node that joins multiple flat parts along edges with weld type, leg size, and
material. Distortion prediction (heuristic first, FEA-backed later via phyz coupling). Generates
the cutlist + weld map automatically.

### 10. AI-native MCP surface

```
sheet_metal.create_base_flange(sketch, thickness, material)
sheet_metal.add_edge_flange(part, edge, length, angle, relief?)
sheet_metal.unfold(part) -> flat_pattern
sheet_metal.check(part, shop_profile) -> violations
sheet_metal.cost(part, shop_profile) -> quote
sheet_metal.suggest_fix(violation) -> ir_patch
```

An LLM with these tools can iterate to a manufacturable part. With `suggest_fix` it can self-heal.
This is something no existing CAD vendor can ship because their kernels aren't structured for it.

## UI: From first principles

### Principles

1. **No modes.** Sheet metal is a property of the part, not a workspace.
2. **The flat pattern is a peer, not a view.** It edits live, beside the 3D.
3. **Direct manipulation beats dialogs.** Click an edge, drag the flange out.
4. **DFM is ambient, not modal.** Lint-style inline marks, like a code editor.
5. **Cost is always visible.** A live number you can't un-see.
6. **Provenance everywhere.** Every bend tells you which row of which table produced its allowance.
7. **Keyboard is a first-class input.**
8. **AI is a participant in the canvas, not a sidebar bolt-on.**

### Core layout

```
┌─────────────────────────────────────────────────────────────────┐
│ FeatureTree │      3D Viewport     │     Flat Pattern           │
│             │  (R3F, ray-traced)   │   (2D canvas, same IR)     │
│             │                      │                            │
│             ├──────────────────────┴────────────────────────────┤
│             │  Contextual Bend Strip (only when SM selected)    │
│             ├──────────────────────────────────────────────────┐│
│             │  PropertyPanel  │  DFM Inspector  │  Cost  │ AI  ││
└─────────────────────────────────────────────────────────────────┘
```

The split viewport is togglable: `Shift+F` snaps between 3D-only, flat-only, and split. The two
windows share camera-anchor selection: click a face in 3D, the corresponding region pulses in the
flat pattern, and vice versa. A thin "crease ribbon" along the bottom of the 3D view shows every
bend in the part as a chip — click to highlight, drag to reorder if topology allows.

### 3D viewport: direct manipulation

- **Hover an edge** → faint perpendicular ghost shows default flange direction. Click-drag → flange
  materializes, length following the cursor, angle snapping to 90/45/30/15° (`Shift` for free angle,
  `Alt` for the other side). Release → an inline pill near the cursor lets you tweak
  `length / angle / radius / relief` without opening a dialog. `Esc` cancels.
- **Hover a bend** → axis renders as a dashed line, radius as a halo. Drag the halo perpendicular
  to the axis → radius changes live with the K-factor pill showing the new BA. Drag along the axis
  → bend angle changes; the flat repaints in real time.
- **Hover a corner** where two flanges meet → relief icon appears. Click cycles styles
  (rectangular, obround, tear, none). The icon only shows when a violation is possible — quiet UI.
- **Hover a face** → if it's a candidate for a hem, jog, louver, or forming tool, those options
  appear in a radial mini-palette at the cursor (`Tab` to summon explicitly).

### Flat pattern editor

Not a thumbnail. A full editor with the same input model as the 3D view.

- Bend lines render as colored creases: red = up, blue = down, with angle and radius labels you can
  scrub.
- **Tooling-only features** (registration holes, nest tabs, fiducial marks, etch text, vendor logos)
  live here and travel with the flat. They render as ghosts in 3D so designers know they exist but
  are not part of the bent geometry. **This is the killer feature**: edit the manufacturing artifact
  without losing it on the next rev.
- **Sheet stock overlay** shows a translucent rectangle of the configured stock size with grain
  direction arrow. Drag the part to reposition; cost updates as utilization changes.
- **Nest preview** shows ghosted other parts in the same job — drag-to-rearrange, with the nesting
  solver re-running on release.
- **Crease constraints**: drag a hole, hold `Cmd`, click a bend line — the hole is now constrained
  "X mm from this bend"; survives radius and K changes.

Same selection state, same undo stack, same IR as 3D.

### Contextual Bend Strip

Appears only when a sheet-metal part is selected, anchored under the viewport. Eight buttons, each
summoning a gesture rather than a dialog:

`Base Flange · Edge Flange · Miter · Hem · Jog · Sweep Flange · Lofted Flange · Forming Tool`

Each shows a tiny live preview of what will be created based on current selection. Hover for
keybinding. Right-click for advanced options.

### Property panel: structured, scrubbable, provenanced

For an `EdgeFlange`:

```
┌─ Edge Flange #3 ──────────────────────┐
│ Length        25.00 mm   ⇕            │
│ Angle         90.0°      ⇕            │
│ Radius        1.50 mm    ⇕            │
│ K-factor      0.42  ●  → table:Al-1mm │
│ Relief        Rectangular ▾           │
│ Springback    +1.2°  (compensated)    │
│ Position      Material outside ▾      │
└───────────────────────────────────────┘
```

Every value is a `ScrubInput`. Provenance dot (●) is colored: green = built-in table,
blue = shop table, purple = measured. Click → jump to the table row.
Length accepts expressions (`thickness * 4`).

### DFM Inspector

Like VS Code's Problems pane:

```
⚠  Hole too close to bend (1.2 mm < 2.5 mm required)
   Sketch: front_face / Hole #4 · Bend #2
   [ Move hole ]  [ Increase bend radius ]  [ Ignore ]
```

Click row → camera flies to the violation in 3D and flat. Each fix button is a real IR patch with
hover preview. `Ignore` adds a justification field (audit trail preserved). The same data backs
the MCP `check_manufacturability` tool.

Header chip shows totals: `0 errors · 3 warnings · 1 ignored`. When green, the part is shop-ready.

### Cost badge

Bottom-right of the viewport, always visible:

```
$4.27 each · qty 100 · 3.2 min · 4'x8' Al 1mm
```

Click → expands to breakdown (material / pierce / bend / setup / margin). Drag the qty number to
scrub volume pricing. A small sparkline shows cost over the last 10 edits — you watch the part get
cheaper as you fix it. Cost as a design **gradient**, not a final reckoning.

### Bend Table editor

A first-class document type, not a modal dialog. Opens in a tab. Spreadsheet UI: rows = thickness,
columns = bend radius, cells = `(BA, K, springback)`. Each cell has a provenance dot and a
"measured-vs-predicted" delta if test data exists. Editing a cell shows which parts in the open
document depend on it and live-updates them.

A "Submit to registry" button packages anonymized rows for the community bend-table repository.

### Shop Profile panel

Lists your brakes, lasers, materials in stock, grain rules, labor rates. Drives DFM checks,
costing, and tool selection. Saved per-user and exportable as a JSON profile.

### Keyboard map

```
B   Base flange (enters sketch)
E   Edge flange on selection
M   Miter flange
H   Hem
J   Jog
S   Sweep flange
L   Lofted flange (with developability error report)
T   Forming tool

F           Toggle flatten preview (ghost)
Shift+F     Cycle 3D / flat / split layouts
U           Toggle unfolded state
D           DFM Inspector
$           Cost panel
N           Open nesting view
Cmd+K       AI command bar
```

Selection-aware: `E` does nothing if no edge is selected and shows a tooltip explaining why. No
silent failure.

### AI integration in the canvas

`Cmd+K` summons a single-line input anchored at the cursor, pre-filled with the selected entity's
name:

```
> add a 25mm flange at 90° to this edge with rectangular relief
```

Submit → emits MCP `sheet_metal.add_edge_flange(...)` → IR op → live preview → `Enter` to commit,
`Esc` to discard. Same mechanism for "make this part cheaper" (returns 3 IR-patch suggestions),
"fix all DFM warnings" (chained `suggest_fix` calls with diff preview), or "convert this assembly
to a single sheet-metal weldment."

The AI sees the same IR + violations + cost the user sees, so it never produces unmanufacturable
suggestions.

### Onboarding

First time a user creates a sheet-metal feature, auto-split the viewport and run a 30-second tour:
click an edge, drag a flange, watch the flat update, see the cost change, see a DFM warning
resolve. No modal dialogs — inline coachmarks that fade once acknowledged.

The default new-document template is a sheet-metal-aware empty doc with a sample shop profile so
the cost badge shows real numbers from second one.

### Visual language

- Bends render with a subtle anisotropic shader hinting at grain direction in 3D.
- Flat-only features render with a dashed outline in 3D.
- DFM violations: amber halos in 3D, dotted underlines on flat dimensions, both pulsing in unison.
- The K-factor provenance dot color is **the same color** across the property panel, bend table,
  and DFM inspector — one visual taxonomy for "where did this number come from."

## Implementation path

### Crates (new)

- `vcad-kernel-sheet` — operations, bend regions, unfold/refold, manufacturability checks
- `vcad-kernel-bend-tables` — table format, registry loader, provenance
- `vcad-kernel-nest` — 2D nesting (rectangle + true-shape via no-fit polygon)
- `vcad-kernel-cost` — process-aware cost model

### Crate edits

- `vcad-kernel-topo`: add `BendRegion` + `Crease` graph; thicken-with-bend-awareness
- `vcad-kernel-sweep`: developable-surface sweep with error metric for lofted flanges
- `vcad-ir`: `SheetMetalOp` variants + `FlatPatternView` IR
- `vcad-kernel-step`: round-trip the metadata
- `vcad/src/export/dxf.rs`: layered output with shop-specific dialects

### App

- `SheetMetalPanel.tsx`, `FlatPatternView.tsx` (live alongside 3D), `BendTableEditor.tsx`,
  `ShopProfile.tsx`, `ManufacturabilityInspector.tsx`, `CostBadge.tsx`
- Toggle: 3D-first vs. flat-first authoring; both edit the same IR

### Data

- Seed `bend-tables/` with public-domain values and a contribution workflow
- Seed `materials/` registry with mechanical properties for springback

## Sequencing

1. **Foundation (2–3 wks).** `BendRegion` topology, `BaseFlange`, `EdgeFlange`, deterministic
   unfold/refold with property tests proving involution within tolerance. DXF flat export. **This
   alone beats most open-source CAD.**
2. **Tables + provenance (1 wk).** Bend table format, default data, per-bend provenance, UI to edit.
3. **Manufacturability checks + Shop profile (2 wks).** Rule engine, live UI, MCP exposure.
4. **Costing (1 wk).** Pure function of IR + shop, badge in UI.
5. **Springback + advanced flanges (2 wks).** Miter, hem, jog, lofted with error report.
6. **Flat-first authoring (1 wk).** UI mode flip; same IR.
7. **Nesting + multi-part DXF (2 wks).**
8. **Weldments (3 wks).**
9. **Community bend-table registry (open-ended).**

Total to legendary MVP: **~12 weeks** of focused work, of which the first 3 already ship something
other open CAD doesn't have.

## UI implementation order (matched to backend tiers)

1. With foundation: `SheetMetalPanel`, contextual Bend Strip, click-drag edge flange gesture, basic
   split viewport with live flat pattern, property panel with scrubs.
2. With bend tables: Bend Table editor tab, provenance dots wired through, shop profile settings.
3. With DFM: DFM Inspector panel, inline violation marks in 3D + flat, fix-suggestion previews.
4. With cost: Cost badge, breakdown popover, per-edit sparkline.
5. With springback + advanced flanges: Hem/jog/miter gestures, lofted-flange developability
   warning UI.
6. With flat-first: Flat-only authoring mode, tooling-only features that ghost in 3D, sheet stock
   overlay.
7. With nesting + multipart: Nesting view, drag-to-rearrange, multi-part DXF export wizard.
8. With weldments: Weldment cutlist + weld map UI, distortion preview overlay.
9. AI command bar can land at any tier; cheapest at tier 3 once DFM is queryable.

## The bet

Existing CAD vendors can't catch up here because their kernels were built before
manufacturability-as-a-typed-query was a design goal, before AI agents needed structured DFM
feedback, and before community data registries were a credible distribution channel. The
combination of **lossless bidirectional unfold + open bend-table registry + DFM-as-MCP** is the
legendary triple. Each is independently useful; together they make vcad the obvious choice for any
shop that touches sheet metal.

The thing nobody else has: **three windows that share state.** The 3D, the flat pattern, and the
bend table are the same model viewed three ways. Edit any of them, the others update with
provenance preserved. Existing tools have these as separate features that occasionally talk; we
make them a single object. That's the UI claim that makes the rest legendary.
