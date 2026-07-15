# Animation: the video rendering engine for agents

vcad documents carry an optional `timeline` — keyframed tracks over named
parameters, joint states, instance visibility, or a global explode factor,
plus declarative camera shots (turntable / orbit / focus). Three MCP tools
drive it:

| tool | role |
|---|---|
| `animate` | author/replace the timeline as data (validated against the doc) |
| `render_sequence` | compile to an **animated GLB** (glTF animation channels) — the cheap preview loop |
| `export_video` | kernel-rendered frames → GIF (always) or MP4 (when ffmpeg is present), with the proof HUD burned in |

The moat: a sequence is evidence, not spectacle. When the document has
`clearance_specs`, both render tools re-measure every spec across the sampled
frames and attach a report — `{label, required_min_mm, observed_min_mm,
worst_frame_t, holds}` — and `export_video` burns the status into every frame
(green ✓ / red ✗, bottom-right).

## Demo

```bash
VCAD_WASM_SKIP=1 npm run build --workspaces --if-present
node examples/animation/gear-train.mjs
```

Builds a 3:1 gear pair on a plate, spins the drive gear one revolution
(pinion counter-rotates 3× via its own track), sweeps a 360° turntable, and
emits `gear-train.glb` (animated, plays in any glTF viewer) and
`gear-train.gif` (49 frames with the HUD: joint readouts left, air-gap
verification right). The receipt reports the 0.5 mm design air gap holding
across all sampled frames.

## Notes

- Joint / visibility / explode tracks become real glTF animation channels on
  the per-instance nodes (geometry evaluated once per partDef).
- Parameter tracks re-evaluate geometry at up to 24 sampled times and switch
  between the samples with STEP visibility channels.
- Turntable/orbit camera compiles to a yaw channel on an invisible
  `__camera` carrier node; the MCP viewer reads it and orbits its own
  camera, so the model stays put while the view sweeps (generic glTF
  players ignore the empty node and just play the model motion).
  `export_video` currently renders the fixed kernel views
  (`iso`/`front`/`side`/`top`).
- Determinism: same document + timeline → same frames. Frame count =
  `round(durationS × fps) + 1`, inclusive of t = 0.
