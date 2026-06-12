# Task assets

Per-task asset files referenced from `inputs[]`. One subdirectory per task.

## Layout

```
assets/<task-id>/
├── host.vcad        # Suite F: host geometry the grader assembles with the accessory
├── target.vcad      # Suite D: target geometry for shape-similarity grading
├── front.jpg        # reference image, looking down +Y at the host
├── side.jpg         # reference image, looking down +X
├── top.jpg          # reference image, looking down -Z
└── hero.jpg         # F3/F4 only — single 3/4-view render
```

## Generated renders (`image_kind: "render"`)

All current reference images are deterministic renders of the checked-in
grader-side geometry (`host.vcad` / `target.vcad`), produced by:

```bash
node mecheval/scripts/render-task-assets.mjs
```

The script drives the `vcad-render` binary (`cargo build -p vcad-render`)
with `--jpeg`, mapping each input's `view` field to a camera: `front`
looks down +Y, `side` down +X, `top` down −Z, `hero` is the 3/4
isometric. Output is 1024×1024 sRGB JPEG (quality 92), part filling ~60%
of the frame, matte neutral background, hidden-line-removed edge overlay,
mild depth cueing.

Renders carry no scale fiducial — scale is conveyed by each task's
`known_dimensions` input instead. Regenerate (and re-commit) the images
whenever a task's host/target geometry changes; renders are
deterministic, so an unchanged geometry produces an unchanged image.

## Photo capture rules (`image_kind: "photo"` — real photographs)

These rules apply to any future checked-in reference *photograph*. The
generation script never touches photo inputs.

- **Fiducial.** Place an ArUco 4x4 50mm marker (id 0) flat on the staging
  surface, in frame, no occlusion. The grader does not consume the
  fiducial — it exists so the agent can recover scale.
- **Background.** Matte, neutral. Avoid surfaces that confuse silhouette
  segmentation.
- **Lighting.** Diffuse, no harsh specular highlights. Two soft sources at
  ±45° azimuth.
- **Camera.** Roughly orthographic feel — long focal length, distant
  camera, host filling ~60% of the frame on the long axis.
- **Pose.** The host is placed in the same coordinate frame declared in
  the task's `host_geometry.frame`. `front.jpg` is the +Y view, `side.jpg`
  is the +X view, `top.jpg` is the -Z view (looking down).
- **Resolution.** Min 1024px on the long axis, sRGB JPEG, quality ≥ 90.

## Host `.vcad` rules

- Single root, single material.
- Coordinate frame matches the `frame` declared in the task's
  `host_geometry` input.
- Geometry is the *as-photographed* part — if the photo shows surface
  imperfections, the host `.vcad` does not need to model them, but the
  nominal dimensions must match what the agent could reasonably recover
  from the fiducial.

## Status

All D1/F1/F2/F3/F4 reference images are generated renders (see above),
checked in alongside the geometry, so the harness runs every task
end-to-end with the `default-cube` baseline (which fails every Fit
check — that's the point). Real photographs can replace renders per-task
later by flipping the input back to `image_kind: "photo"` and following
the capture rules.
