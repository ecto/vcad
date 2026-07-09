# vcad brand + UX specification

Status: locked (rev a). Owner: cam. 2026-07-08.
Produced in the landing-page/brand session; canonical reference for vcad.io, docs.vcad.io, app.vcad.io, and the native app.

One sentence over everything: **close the gap between designed and built — and prove it closed.**

Parked (explored, not canon): the 8-rule "shop rules" constitution; the DRO scroll
readout; a rename (see § Naming).

---

## 1. The mark

**V1 — a filled inverted triangle.** No interior detail, one color.

Readings, stacked (never explained in marketing; they're for us):
the **v** of vcad · **∇** the gradient operator (the differentiable kernel) ·
a **cutting tool entering stock** · a **V-block** · one stroke from a checkmark.
Positioning: *Vercel points at the cloud; we point at the stock.*

- **V3 variant** — the hollow nabla outline — is the formal/print variant only:
  spec covers, drawing watermarks, laser etching. Filled = product; outline = paper.
- The mark is a real document: **`logo.vcad`** (a triangular prism). CI re-renders
  the SVG/favicon via `vcad-render` on release. The logo has a mass, a receipt,
  and a STEP export.

## 2. The wordmark — ▽cad

The triangle **is** the letter v. One asset serves as symbol (alone) and wordmark (in flow).

Optical spec (these are parameters in `logo.vcad`):

| parameter | value | why |
|---|---|---|
| height | 1.045 × x-height | it's a lowercase letter, plus overshoot |
| baseline overshoot | +4.5% | pointed forms dip below the line; the tool lands |
| width | 1.20 × Inter v, then −2% | solid shapes read narrower, then heavier |
| apex angle | 63.5° | falls out of the above; kin to Inter's v |
| gap to c | v–c kern + 0.02em (≈0.055em) | reads as a word, breathes past the mass |
| letters | Inter 700, −3% tracking | display letters, not custom |
| size floor | 14px | below it, triangle alone |

Never: orange the triangle in the lockup · outline/rotate/shadow it · write "▽ + vcad"
(double-spelling). Body text spells vcad normally.

## 3. Type

- **Inter** — display 700 at −5% tracking; UI 400/500.
- **JetBrains Mono** — the spec layer: every number, receipt, eyebrow, terminal, code.
- Sentence case everywhere, including eyebrows (`§01 · Getting started`). No all-caps copy.

Micro-typography (numbers are the product):
tabular figures everywhere · true − ± ø × · thin space before units and as thousands
separator · a number never wraps away from its unit · every displayed dimension is
hoverable and knows its provenance.

## 4. Color law

Two accents with fixed meanings. They never swap and are never decorative.

| color | value (dark / light ctx) | means | examples |
|---|---|---|---|
| orange | #F25C1F | **interaction / attention** | CTA links, active tool, selection, dimension callouts, weakest link, running check |
| green | #3ECF8E / #00875C | **verification** | receipts, passing checks, "Fully constrained" |

Surfaces: carbon `#0A0A0B` (site + app), panel `#0F1013`–`#141519`, hairlines `#1A1B1E`.
Text: `#F5F5F6` display, `#E6E7E9` primary, `#7A7E86` secondary, `#3A3D42` whisper.
Light contexts (docs body, print): ink `#16181D` on white, hairlines `#E8EAED`.

Structure: **radius 0 everywhere. No filled cards — hairlines only.** The signature
accent is the **corner tick** (blueprint corner): featured items, focus states,
drawing-sheet frames. No gradients, no shadows, no cream.

## 5. Motion

Two easings, from G-code. Two durations. Nothing else.

- **G0 rapid** — 90ms, near-linear, hard stop. Chrome: panels, palette, tree.
- **G1 feed** — 240ms, constant rate. Geometry: previews, boot, the ring closing.

Banned: bounce, spring, ease-in-out, parallax, anything cute.
Boot: the triangle descends 8px to baseline touch-off (G1), `cad` types after it (G0).
Once per visit, never blocking content.

## 6. Sound (optional, off by default)

- ⏎ commit → relay click (8ms)
- ring closes / provable → single machined-steel tap
- check fails → dull low thud (failure isn't musical)

## 7. Voice

Shop voice: terse, factual, zero enthusiasm inflation.
- Praise is a passing check ("9 of 9 checks pass. Provable.")
- Errors name the geometry and offer the next move ("Boolean failed. Faces 12 and 31
  don't close. Show me →")
- No exclamation points, no emoji, no "simply".
- Exactly one wink in the whole surface area: the 404 — "offcut: this page fell off
  the sheet."

## 8. Render art direction

One locked camera (35°/30°). One key light, high left, hard shadow, near-black void.
The ray tracer is the photographer. Orange exists in renders only as annotation
(dimensions, section arrows) — never on the part. No lifestyle, no hands, no desks.

## 9. vcad.io — the landing page (9-frame storyboard, approved)

The plan: vcad.io becomes this landing; the app moves to **app.vcad.io**; the page
lives in a new **`packages/landing`** (independent deploy).

| # | frame | content | motion |
|---|---|---|---|
| 01 | boot | triangle touch-off → `kernel ready` | G1, 300ms, once |
| 02 | hero | **"Design it. Prove it. Make it."** / "Sketch it at breakfast. Hold it on Thursday." / [Start designing] [Open in Claude ▾] / hero part render + live receipt line | static |
| 03 | triptych | Design it (viewport) · Prove it (checks) · Make it (quote) — intro: "From napkin sketch to anodized aluminum, with a receipt at every step." | G0 reveal, 90ms stagger |
| 04 | demo | agent transcript types itself: bracket · 40 N · 14 rules · fea 2.1× · $23.40 · make it? ⏎ | ~6s, skippable — the page's only theater |
| 05 | gallery | "Made with vcad. Measured after." + "The bracket, the enclosure, the instrument you keep meaning to build — build it." | static |
| 06 | receipt | "Nothing ships unproven." — seal ring closes | scrubbed to scroll |
| 07 | deal | "Free to design. 3% when you make." | static |
| 08 | agent close | "Your agent already knows how to use it." + Open in Claude ▾ | static |
| 09 | title block | page = drawing: title / drawn by humans + agents / rev (package.json) / checked (✓ ci) | measured-headline detail |

Cut grammar between sections: orange hairline + mono callout (`cut a`), content below
shifts 6px (the sheet moved when the part was released).

**Gallery honesty rule:** captions may claim only physically measured results. The
fork/glockenspiel/motor captions shown in design are *target state* — verify physical
status at build time or the item waits. (This gives the cornerstone projects a
marketing deadline.)

**Agent CTA:** no `npx` in the hero. Split button "Open in Claude" (Claude mark,
orange outline) with dropdown: Claude Code (cli) · Cursor (deep link) · ChatGPT
(connector) · "Copy setup for anything else" → plain MCP config + one-line
instructions. No incantations.

Copy deck placement: "CAD that proves its work." → og/meta + README ·
"14 checks between you and a bad part." → checks section ·
"Nothing ships unproven." → receipt section ·
"You describe it. The kernel checks it. A machine cuts it. You bolt it on." → docs/demo.

Not on the page: feature grid, testimonials, logo wall, FAQ. The gallery with
instrument-measured captions is the social proof.

Pricing page (separate): Design $0 · Make 3% of orders · Team $20/seat, hairline
table, corner ticks on the featured tier. Changelog page: a rev table.

## 10. app.vcad.io + native macOS — the one-room app

Native-first: the web app is a faithful port of the macOS layout, not vice versa.
Full detail in memory (`vcad-app-ux-north-star`); summary:

- **No modes.** Title bar: filename, ⌘K, check count, Make it. Sketch is the only
  entered scope (breadcrumb + Esc; solver status is chrome). Assembly emerges from
  selection. Draw/Simulate/Export are **outputs** inside the Make-it flow.
- **Zero chrome resting state:** the part + Make it + check count + status whisper
  (`gripper · 96.2 g · $38.10 · ⌘K for anything` — live mass and price, always).
- **Summoned surfaces** (appear at the work, leave; pinnable to dock): tree over
  glass (left edge / ⌥T), inspector attached to selection, agent (⌥Space).
- **Universal contracts:** preview-before-commit for every mutation (human, gesture,
  agent) with consequence line ("If applied: ✓ dfm passes · mass −1.3 g");
  "type anywhere, numbers commit"; gestures navigate, keys commit, nothing
  destructive on a gesture.
- **The seal** (behind a flag; ship `✓ n/n` + Make it first): ring = proof, interior
  = action, button born when ring closes; stale edits dash the ring; ordering fills
  an inner orange arc. Receipt rows and geometry share one selection model.
- **Tower UI:** scale-rail sliver (⌥scroll descends; ships as a setting until it
  demos), trust as glyphs ■ ● ◐ ○ (never color), weakest link in orange with
  simulate-vs-measure remediation card, as-designed | as-built viewport toggle,
  σ in the status bar.
- Empty document: two verbs — **Sketch** / **Describe it** — plus "drop a step file
  anywhere."

## 11. docs.vcad.io

Light sibling of the landing. Mono section anchors, terminal blocks in app-carbon,
⌘K, right-rail "On this page", title-block footer with **"checked ✓ examples ran in
ci"** — build this for real: every code sample on a page actually executed.

## 12. Institutions (locked)

1. **The State of the Gap** — annual public report: predicted vs measured across
   every order the pipeline shipped; error-bar coverage graded in public.
2. **Postmortems as NCRs** — incidents written as non-conformance reports: root
   cause, containment, corrective action, verification.
3. **The dimensioned UI** — hold ⌥ anywhere (app, landing, docs): the interface
   annotates itself with its own measurements, drawing-style.
4. **Every release is a part** — each major rev ships a physical first article
   exercising the headline feature, photographed on the release notes with receipt.
5. **First Article certificates** — a user's first fabricated part earns a numbered
   certificate (First Article №——) with its receipt.
6. **The spec verifies itself** — brand constants live in `logo.vcad` + `brand.toml`;
   CI runs `verify_spec` on the brand. Off-brand is a failing check.

## 13. Naming

Staying **vcad**; the system above is name-portable by design. Three rounds ran
(2026-07-08). Best word found: **kerf** (kerf.com for sale on Afternic; npm `kerf` +
`kerf-mcp` free; risk: kerf.works). Runners-up: qed (the proof tombstone ∎ — kept as
candidate name for the receipt format), swarf.com / detent.com (for sale, banked).
Dead: tesseract (OCR owns it; its hypercube survives as the science illustration
set), manifold (our own neighbor library), form (generic). Untried direction: coined
words constrained by letterform geometry. Full archive in project memory.

Vocabulary banked for features: **witness** (receipt attestations), **detent**
(pinned checkpoints), **first article** (onboarding), **gage** (calibration).

## 14. Site information architecture — the tagline is the sitemap

Apple's product-site rules, adopted: the nav names the product's anatomy (never
departments); the homepage is one chaptered story; depth is one door per chapter
("→"), never a detour; dense facts get a beloved specs page. No mega-menus, no
"Solutions", no blog in the nav.

```
vcad.io/            the story — nine frames (§9), unchanged; gains 4 quiet → doors
  /design           one document, every domain — "the blank page isn't blank"
  /prove            THE MOAT PAGE — "nothing ships unproven" (storyboard below)
  /make             quote/order/export + the gallery of measured things — "it ends at the bench"
  /agents           audience page — your agent's first CAD seat, 140 tools, transcripts
  /kernel           the specs shrine — ops, formats, tolerances, honest benchmarks; dense mono
  /pricing          free to design · 3% when you make · team
  /changelog        rev table          /company    the title block, expanded
app.vcad.io works · docs.vcad.io teaches · mcp.vcad.io serves agents
```

Nav (7 items total): Design · Prove · Make · Agents · Pricing · [Open in Claude]
[Web app]. Docs lives in the footer and /kernel, not the nav. Homepage chapter
doors: triptych→/design, receipt→/prove, gallery→/make, agent close→/agents.
/prove gets the most craft — it's the page no competitor can have. A compare
page is deliberately deferred until the gallery has enough measured artifacts.

### /prove storyboard (8 frames, approved)

01 hero "Nothing ships unproven." — reel background weighted toward the green
verified frame. 02 the indictment — "Every CAD tool stops at the file. You find
out at the machine." two sentences, no illustration. 03 **break the part** —
the page's one piece of theater: a live kernel part with one wall-thickness
slider; drag it thin, watch `min_wall` fail in red with the fix offered. The
visitor personally fails DFM and gets saved. 04 anatomy of a receipt — real
`vcad.receipt/1` JSON annotated like a drawing (claims / checks / provenance /
verdict); link the schema; recruits builders. 05 fail-closed, three laws —
zero claims never passes · unverifiable never passes · edited goes stale; seal
lifecycle strip beneath. 06 every number knows its parents — trust glyphs
■ ● ◐ ○; on this page the dimensioned-UI hover shows provenance chains for the
page's own numbers; one tower tease line. 07 the loop closes — predicted vs
measured coupon data (HONESTY GATE: real instrument data only; sim-vs-sim
labeled as such until the fork rings); ends on the State of the Gap promise.
08 "Prove yours." + title block, whose Checked cell is load-bearing here.
