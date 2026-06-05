# Electronics workspace UI consolidation

**Date:** 2026-06-05
**Goal:** Collapse the electronics workspace's scattered floating chrome into two
homes: the left property panel and the top borland tool palette.

## Decisions

1. **Single "Circuit" tab** in the top `ToolPalette` (not a context-swap of the
   whole strip). The mechanical tabs stay; one new `circuit` tab holds the
   electronics tools. Schematic vs board stays on the top-center view toggle.
2. **Persistent status header** in the left panel: mode·tool, DRC/ERC chips,
   active-layer dot, and the board-overview counts sit above whatever is
   selected (an ECAD item, or the board transform when nothing is).

## Task 1 — Panel consolidation

Move two floating HUDs into the left `PropertyPanel` (the panel that already
shows the PcbBoard part + "Edit Circuit"):

- `ElectronicsStatusPanel` (top-left: mode/tool, DRC/ERC, active layer)
- `ElectronicsPropertyPanel` (top-right: "Board Overview" counts + health)

New layout when electronics is active:

```
┌ PCB Board ───────────────── [Edit Circuit] ┐
│ PCB · Select    DRC 1 ⚠0    ● F.Cu          │  ← status header (new)
│ Footprints 2  Traces 1  Vias 0  Nets 2 …    │  ← board overview (moved)
├──────────────────────────────────────────────┤
│ selected-item inspector (EcadFeatureInspector)│
│ or board position/rotation when none selected │
└──────────────────────────────────────────────┘
```

Then delete the two floating mounts (`App.tsx`, `Viewport.tsx`).

## Task 2 — Circuit tab

- Add `circuit` to `ToolbarTab` (ui-store) + tab colors/themes/descriptions +
  `getAllTabs()`.
- Render the electronics tools when `toolbarTab === "circuit"`, context-aware by
  `electronicsLayout` (schematic vs board), reusing the existing render logic
  from `ElectronicsToolbar`.
- Couple `toolbarTab === "circuit"` ⟺ electronics `active`: selecting Circuit
  enters (or offers "New PCB Board"); selecting another tab exits; `enter()`
  also pins the tab to `circuit`.
- Remove the bottom `ElectronicsToolbar` mount.

## Out of scope

- The top-center Schematic⇄Board toggle stays as-is.
- No kernel / IR changes; pure app-layer UI.

## Verification

App typecheck + tests, then click-through in the preview: enter via Circuit tab,
confirm tools switch with the view toggle, panel shows live status + counts, and
leaving via another tab exits.
