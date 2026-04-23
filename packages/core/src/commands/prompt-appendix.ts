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
5. \`tube\` for handlebar, seat post, crank arm.
6. \`set_material\` on each part.
7. \`screenshot_viewport({ view: "quad" })\` to verify.

This pattern generalizes: whenever you'd be tempted to compute perpendicular
vectors or tube lengths by hand, stop and use tube / polyline_tube instead.`;
