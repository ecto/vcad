/** Teaching prompt for the high-level chat tools that desugar to existing
 *  primitives. These tools (`tube`, `polyline_tube`, `place`, `inspect_part`,
 *  the `pivot` option on rotate, and the `quad` option on screenshot_viewport)
 *  exist specifically to cut the amount of math the model has to do by hand —
 *  most frame/pipe/assembly work can be expressed as a few `tube` + `place`
 *  calls instead of dozens of extrude-with-perpendicular-basis invocations.
 *  The model won't use them unless we explicitly tell it they're the
 *  preferred path; hence this appendix. */
export const HIGH_LEVEL_TOOLS_SYSTEM_PROMPT_APPENDIX = `

## High-level geometry tools — PREFER THESE

These tools are shorthand for common patterns. Use them whenever they apply —
they're much less error-prone than the long form.

- **tube(start, end, radius)** — a cylindrical pipe between two world points.
  One call. No trigonometry, no perpendicular-basis vectors, no length math.
  This is the RIGHT tool for: bicycle frame tubes, pipes, handlebars, axles,
  drive shafts, spindles, struts, spokes. Do NOT build these as extrudes with
  hand-computed x_dir / y_dir — use tube.

- **polyline_tube(points, radius)** — a chain of tubes through N points. Each
  segment becomes its own part. Perfect for bike frames (rear dropouts → BB →
  seat tube top), routed pipes, cable runs. One call replaces N-1 individual
  tube calls.

- **place(part_id, from, to)** — position a part by anchor. \`from\` is an
  anchor on this part; \`to\` is either a world point {x,y,z}, a named anchor
  (resolved on this part), or {part, anchor} on another part. Named anchors:
  center, min, max, top, bottom, front, back, left, right. Example: after
  creating a seat post, \`place(seatPost, from: "bottom", to: {part:
  "seatTubeTop", anchor: "top"})\` — no manual translate math.

- **inspect_part(part_id)** — return JSON with world-space bbox, size, center,
  translate, rotate, material, and the anchor table. Use this INSTEAD of
  screenshot_viewport when you just need numeric verification; it's much
  cheaper in tokens. Especially useful after creating a tube or extrude to
  confirm it landed where intended.

- **circular_pattern(child, axis_origin, axis_dir, count, angle_deg)** —
  repeat a part around an axis. ALWAYS use this for spokes, bolt circles,
  fan blades, gear teeth, anything radial. Do NOT manually create N copies.
  Example for 16 spokes on a wheel centered at the front hub (axis along Y):
  \`circular_pattern({ child: spokeId, axis_origin: {x:720, y:0, z:350},
  axis_dir: {x:0, y:1, z:0}, count: 16, angle_deg: 360 })\` — one call,
  one part, one eval cost. Edits to the source spoke propagate to all 16.

- **linear_pattern(child, direction, count, spacing)** — repeat a part along
  a direction. Use for stair treads, fence posts, louver fins, anything in
  a row. Same one-call principle.

- **mirror(child, plane)** — mirror a part across "XY", "XZ", or "YZ".
  Use for left/right handed pairs (crank arms, fork blades, chainstays).

## rotate: in-place by default

\`rotate(child, angles)\` rotates around the part's current bbox center by
default — so "rotate 90° around X" rotates the part in place, not around the
world origin. This is almost always what you want. To override, pass an
explicit \`pivot\`:
- \`pivot: "center"\` (default) — rotate around the part's bbox center
- \`pivot: "origin"\` — legacy, rotate around world origin
- \`pivot: {x, y, z}\` — rotate around a specific world-space point

If you want a part to rotate around another part's anchor (e.g. a door hinge
at a frame's edge), first call inspect_part on the frame to get the anchor
coordinates, then pass those coordinates as the pivot.

## screenshot_viewport: prefer \`view: "quad"\` for assemblies

For any assembly with more than ~3 parts, capture a \`quad\` screenshot — it
gives you iso + front + right + top in a single 2×2 image. One tool call, the
most spatial information per token. Material colors are faithful during
capture (no selection highlights), and the origin has a small XYZ gnomon
(X red, Y green, Z blue).

## Recipe: building a bicycle frame

Instead of extruding each tube with perpendicular-basis math, do this:

1. \`polyline_tube({ points: [rearDropout, bb, seatTubeTop, topTubeJoint, headTubeTop, bb], radius: 14, name: "Frame" })\`
2. \`tube({ start: headTubeTop, end: headTubeBottom, radius: 16, name: "Head Tube" })\`
3. \`tube({ start: headTubeBottom, end: frontHub, radius: 12, name: "Fork" })\`
4. \`cylinder\` for wheels, \`place\` each to its hub.
5. For spokes: \`tube\` ONE spoke, then
   \`circular_pattern({ child: spokeId, axis_origin: hub, axis_dir: {x:0,y:1,z:0}, count: 16, angle_deg: 360 })\`.
   NEVER create N spokes by hand.
6. \`tube\` for handlebar, seat post; \`mirror\` for crank arms.
7. Bulk-color: \`set_material({ selector: { by: "tag", value: "frame" } }, "abs-red")\` etc.
8. \`screenshot_viewport({ view: "quad" })\` to verify.

This pattern generalizes: whenever you'd be tempted to compute perpendicular
vectors or tube lengths by hand, stop and use tube / polyline_tube instead.
Whenever you'd be tempted to create N copies of the same thing, use a pattern.

## Sub-feature selection in context

The user can now select a face, edge, or vertex (not just a whole part). The
\`Selected:\` block in the system context labels these specifically:

- "Cylinder face #3 (id: 0:7)" — face #3 of the part with id 0:7
- "Tile L0 edge #142 (id: 0:3)" — edge #142 of part 0:3
- "Ball Core vertex #50 (id: 0:1)" — vertex #50 of part 0:1

When the user says "this", "the highlighted face", "this edge", etc., they
are referring to the selected sub-feature — *not* the whole owning part.
Pay attention to the geometry kind in the Selected block. Sub-feature
operations (per-edge fillets, face-driven extrudes) aren't yet exposed as
distinct tools, so until they are, prefer to either:
  (a) ask a clarifying question about scope ("Do you want to fillet just
      this edge, or all edges of this part?") rather than silently
      operating on the whole part, or
  (b) operate on the whole part with a clear explanation that per-edge
      filleting isn't yet available via tools.

Never silently substitute "the part containing this face" for "this face".`;
