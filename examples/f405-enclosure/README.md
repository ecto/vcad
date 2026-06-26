# F405 flight controller, co-verified inside its case

The north-star demo for vcad's **cross-domain verification axis**: a 36×36 mm
STM32F405 flight controller and the 3D-printable case it ships in, built in one
process — the board on the PCB engine, the case on the BRep CAD kernel — and
then **cross-checked against each other**.

No EDA tool (KiCad, Flux, JITX, Quilter, Altium, Cadence) can do this, because
none of them owns a real solid-modeling kernel next to the board. vcad does, so
it can answer the questions a fab house can't:

| Check | What it proves |
|-------|----------------|
| **Board fits with clearance** | the outline sits inside the cavity with margin on every wall |
| **Components clear the lid** | the tallest part's stack height fits under the rim (cavity depth) |
| **Mounting holes land on standoffs** | every M3 hole drops onto a case boss, not into thin air |
| **Connectors align to wall cutouts** | the USB-C port lines up with the opening in the wall |

## Run it

```bash
# From the repo root (fresh worktree? run `npm ci` first):
npm run build --workspaces        # build @vcad/engine, @vcad/core, @vcad/mcp …
node examples/f405-enclosure/build.mjs
```

Outputs land in [`out/`](out/):

```
[3] Enclosure fit: PASS — 4/4 checks (board fits, components clear the lid,
                                       holes land on standoffs, connectors align)
    cavity 38.5×38.5mm × 14.3mm deep · 4 standoffs · 1 wall cutout
    ✓ Board fits cavity with clearance: 1.25mm worst-case clearance (need 0.5mm)
    ✓ Components clear the lid: tallest part U1 leaves 7.6mm under the lid
    ✓ Mounting holes land on standoffs: all 4 align (worst offset 0.22mm)
    ✓ Connectors align to wall cutouts: all 1 connector lines up
```

## What ships

| File | Domain | Format |
|------|--------|--------|
| `*.gbr`, `*.drl` | board | Gerber + Excellon drill — hand to any PCB fab |
| `f405-case.stl` | case | mesh — slice and 3D-print directly |
| `f405-case.glb` | case | mesh — drop into a 3D viewer |
| `f405-case.vcad` | case | editable vcad source (parametric) |
| `verification.json` | both | the cross-domain verdict, line by line |

> STL/GLB are the print-ready mesh formats. STEP (B-rep) export of boolean
> bodies is a kernel TODO — the booleans are evaluated to a mesh today, so the
> case ships as STL, which is exactly what a 3D printer consumes.

## How it works

[`geometry.mjs`](geometry.mjs) is the single source of truth for the case — an
open-top tray whose standoffs are carved *subtractively* out of the interior
pocket (one Difference, no Union seam) so the kernel mesh stays clean. It also
exports the 30.5 mm M3 hole pattern and USB-C edge position the board reuses, so
the two domains are derived from the same numbers by construction.

[`build.mjs`](build.mjs) drives the whole pipeline with the same tools an AI
agent calls over MCP:

1. `open_document` — load the case solid (BRep CAD session).
2. `create_schematic` → `place_components` → `set_placement` — lay out the board.
3. `route_nets` → `run_drc` — route and check copper.
4. **`check_enclosure_fit`** — the cross-domain check. It extracts the case
   cavity, standoffs, and wall cutouts straight from the solid's mesh (via a
   generalized-winding-number occupancy sample, robust to imperfect CSG meshes),
   then verifies the four axes above.
5. `build_receipt` — a durable proof that now carries the enclosure-fit verdict
   alongside the DRC summary: *board passes DRC **and** fits its case*.
6. `export_gerber` + `export_cad` — fab outputs for both domains.

## The MCP tool

`check_enclosure_fit` is a first-class MCP tool — point it at a board session
and a CAD session holding the enclosure:

```jsonc
check_enclosure_fit({
  document_id: "<board session>",
  enclosure_document_id: "<case CAD session>",
  clearance: 0.5,          // mm, optional
  derive: true             // also return an outline + holes seeded from the cavity
})
```

It returns a per-check verdict with measurements, and the same verdict can be
folded into `build_receipt` by passing `enclosure_document_id` there.
