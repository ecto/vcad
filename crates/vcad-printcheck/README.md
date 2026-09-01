# vcad-printcheck

Printability lint for an **exported** mesh, in a chosen print orientation.

```
vcad check part.stl
vcad check part.stl --orientation -z --nozzle 0.4 --max-bridge 4 --crack-threshold 0.15
vcad check shell.stl --allow-bridge 1.75:2.65 --allow-bridge 25.6:26.25
vcad check part.stl --json
```

Exit code is `0` when clean and `1` when anything failed, so it gates CI.

## Why it reads the export

FDM printability is a property of the shipped file, not of the model that
produced it. The rana `60c` shell passed its author's analytic profile
verification while the discretised STL carried 0.05 mm cracks across 85% of its
circumference; only rays cast at the export caught it. Every check here reads
triangles.

The three field-proven checkers this ports —`support-check.py`,
`slice-check.py`, `manifold-check.py` in the rana repo — were each written
after a print failure or a slicer warning that geometry-level checks missed.

## What it reports

| check | what it catches |
| --- | --- |
| floating regions | material starting in mid-air with nothing beneath or beside it |
| interior cracks | material gaps below the threshold (default 0.15 mm) — a slicer reads these as floating layers |
| bridge spans | unsupported spans with lengths, against the 4 mm convention |
| overhang census | downward face area by bucket, with the staircase-vs-support verdict |
| min wall / min feature | thinnest wall against the nozzle, measured along all three axes |
| manifold + sections | bad-edge census, and whether every z-section closes |

## The parts that are easy to get wrong

**Parity, not winding.** Material spans come from strict crossing parity, which
is what a slicer's mesh analysis sees. Parity reads a z-overlap between two
*separate* bodies as an interior void — a defect worth flagging (rana finding
#11 was a rim chamfer ring overlapping the sector columns by 0.05 mm). An
earlier version of the rana checker used a winding sum with an inverted sign,
saw no material anywhere, and passed vacuously for a whole revision. Hence:

**Known-bad fixtures.** `tests/known_bad.rs` asserts the checker *fails* a
0.05 mm crack, a mid-air island, a 0.2 mm wall at a 0.4 nozzle, a 12 mm bridge,
and a holed cube — each with the specific diagnosis, not merely a non-zero
exit. `tests/real_world.rs` asserts the shipped rana shell passes, *and* that
removing its bridge waiver makes it fail, so the waiver cannot hide a broken
check.

**Sampling pitch follows the nozzle.** Rays are cast on a square grid whose
pitch defaults to the nozzle width, not on a fixed column count over the
bounding box. A 70 mm part sampled 64 columns wide puts 1.1 mm between rays,
which straddles a 2 mm tube wall: neighbours land in air, roofs read as
unanchored, and spans get measured across the bore.

**Bridge span is distance to an anchor, doubled** — not the region's own
footprint. A roof over a slot is unsupported across the slot but continuous
along it, and its bounding diagonal would report the length of the slot rather
than the span the extruder has to cross.

**Three filters keep min-wall honest.** A ray must hit the surface within 60°
of head-on (a grazing chord measures the silhouette); the faces at the two ends
of the span must oppose each other (a chamfer feathering to an edge is not a
thin wall); and the voids either side must be real, not hairlines (the rana
shell is meshed as 0.5° sector prisms whose seams parity reads as 0.06 mm gaps,
which would otherwise turn a solid 2 mm tube into a stack of 0.24 mm "walls").

**A crack is never waivable.** `--allow-bridge Z0:Z1` accepts documented
unsupported spans in a height range — the rana shell's slot roofs are accepted
exactly this way. It has no effect on cracks.
