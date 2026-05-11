# Task assets

Per-task asset files referenced from `inputs[]`. One subdirectory per task.

## Layout

```
assets/<task-id>/
├── host.vcad        # Suite F: host geometry the grader assembles with the accessory
├── front.jpg        # reference image, looking down +Y at the host
├── side.jpg         # reference image, looking down +X
├── top.jpg          # reference image, looking down -Z
└── hero.jpg         # F3/F4 only — single 3/4-view photoreal render
```

## Photo capture rules (F1, F2 — real photographs)

These rules are enforced on every checked-in reference photograph. Renders
(F3, F4) follow the same framing but are produced from `host.vcad` via a
fixed render pipeline (TBD).

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

Photographs are captured ad-hoc as F-suite tasks land. The host `.vcad`
is committed at task-creation time so the grader can run end-to-end with
the `default-cube` baseline (which will fail every Fit check — that's the
point).
