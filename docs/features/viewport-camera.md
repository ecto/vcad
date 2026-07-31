# Viewport & Camera

3D navigation and visualization system for viewing models from any angle with intuitive controls.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. All listed implementation files exist (`Viewport.tsx`, `ViewportContent.tsx`, `TransformGizmo.tsx`, `GridPlane.tsx`, `camera-settings-store.ts`, `useCameraControls.ts`, `camera-controls.ts`). Note: a ray-traced viewport render mode also exists beyond what this doc covers.

## Problem

CAD users need to view their models from any angle to:

1. Inspect geometry from different perspectives
2. Select faces, edges, and vertices accurately
3. Verify design intent before manufacturing
4. Present designs to stakeholders

Without robust 3D navigation, users struggle to:

- Rotate around complex assemblies
- Zoom into fine details
- Pan to different areas of large models
- Quickly snap to standard views (front, top, iso)

Traditional CAD tools require learning complex mouse+modifier combinations that vary between applications (Fusion 360 vs SolidWorks vs Onshape), creating friction for users switching between tools.

## Solution

Full viewport system with configurable camera controls and visual manipulation tools:

### Camera Controls

Three primary navigation modes with configurable input bindings:

| Action | Default (vcad) | Alt (Fusion-style) |
|--------|----------------|-------------------|
| Orbit | Middle-drag or two-finger swipe | Shift+Middle-drag |
| Pan | Shift+Middle-drag | Middle-drag |
| Zoom | Scroll wheel | Scroll wheel |

### Smooth Camera Animation

Camera transitions use quaternion slerp interpolation to avoid gimbal lock and ensure smooth, predictable motion when:

- Selecting a part (camera animates to frame selection)
- Clicking gizmo faces to align view
- Snapping to standard views (front, top, iso, etc.)
- Double-clicking canvas to reset view

### Saved Camera Views

Predefined snap views accessible via gizmo clicks or keyboard shortcuts:

| View | Camera Position |
|------|-----------------|
| Front | (0, 0, 80) |
| Back | (0, 0, -80) |
| Right | (80, 0, 0) |
| Left | (-80, 0, 0) |
| Top | (0, 80, 0) |
| Bottom | (0, -80, 0) |
| Iso | (50, 50, 50) |
| Hero | (60, 45, 60) - presentation angle |

### Display Options

| Option | Description |
|--------|-------------|
| Wireframe mode | Toggle edge-only rendering |
| Grid display | Configurable ground plane grid |
| Axis display | RGB orientation gizmo (bottom-right) |

### Transform Gizmo

Visual manipulation widget for selected objects:

- Translate mode: Move along X/Y/Z axes or planes
- Rotate mode: Rotate around X/Y/Z axes
- Scale mode: Uniform or axis-specific scaling
- Grid snapping: Configurable increment (default 5mm)

### Gizmo Face Selection

Click any face of the orientation gizmo to:

1. Swing camera to view that face flat
2. Center on current selection
3. Maintain level horizon (zero roll)

### Snap Settings

| Setting | Default | Description |
|---------|---------|-------------|
| Grid snap | On | Snap transforms to grid increment |
| Point snap | On | Snap to geometry vertices |
| Snap increment | 5mm | Grid spacing |

## UX Details

### Interaction States

| State | Behavior |
|-------|----------|
| Idle | Standard cursor, ready for selection |
| Orbiting | Grabbing cursor, camera rotates around target |
| Panning | Move cursor, camera slides parallel to view plane |
| Zooming | Zoom toward cursor position (not center) |
| Gizmo drag | Transform widget active, orbit controls disabled |

### Camera Animation

| Trigger | Animation |
|---------|-----------|
| Part selection | Smooth zoom/pan to frame selected geometry |
| Face click (gizmo) | Swing to face-aligned view with zero roll |
| Snap view command | Animate to predefined camera position |
| Double-click (empty) | Reset to initial isometric view |

### Edge Cases

- **Looking straight up/down**: Uses world Z as reference to avoid singularity
- **Very large models**: Camera far plane extends to 10000 units
- **Very small details**: Near plane at 0.1 units with logarithmic depth buffer
- **During animation**: Expensive effects (AO, shadows) disabled for FPS

## Implementation

### Files

| File | Purpose |
|------|---------|
| `packages/app/src/components/Viewport.tsx` | Canvas setup, box selection handler |
| `packages/app/src/components/ViewportContent.tsx` | Camera controls, lighting, scene rendering |
| `packages/app/src/components/TransformGizmo.tsx` | Translate/rotate/scale widget |
| `packages/app/src/components/GridPlane.tsx` | Configurable ground plane grid |
| `packages/app/src/stores/camera-settings-store.ts` | Control scheme persistence |
| `packages/core/src/stores/ui-store.ts` | UI state (wireframe, snap settings, etc.) |
| `packages/app/src/hooks/useCameraControls.ts` | Keyboard shortcuts for camera |
| `packages/app/src/lib/camera-controls.ts` | Control scheme utilities |
| `packages/app/src/types/camera-controls.ts` | Type definitions and presets |

### Architecture

The viewport uses React Three Fiber with drei helpers:

1. **OrbitControls**: Base camera manipulation (modified for custom scroll handling)
2. **GizmoHelper/GizmoViewport**: Orientation indicator with click-to-snap
3. **TransformControls**: drei wrapper for translate/rotate/scale gizmo
4. **Custom wheel handler**: Unified zoom/pan/orbit with configurable bindings

### Key State

```typescript
// ui-store.ts
interface UiState {
  showWireframe: boolean;
  gridSnap: boolean;
  pointSnap: boolean;
  snapIncrement: number;
  isDraggingGizmo: boolean;
  isOrbiting: boolean;
  transformMode: "translate" | "rotate" | "scale";
}

// camera-settings-store.ts
interface CameraSettings {
  controlSchemeId: string;
  inputDevice: "auto" | "mouse" | "trackpad";
  zoomBehavior: {
    zoomTowardsCursor: boolean;
    invertDirection: boolean;
    sensitivity: number;
  };
  orbitMomentum: boolean;
}
```

### Animation Algorithm

Camera transitions use spherical interpolation for orientation:

1. Compute goal quaternion from target position (with zero roll)
2. Each frame: `camera.quaternion.slerp(goalQuat, 0.1)`
3. Simultaneously lerp position and target
4. Stop when position delta < 0.1 units

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Orbit camera with middle-mouse drag or trackpad two-finger
- [x] Pan camera with Shift+middle-drag
- [x] Zoom with scroll wheel toward cursor position
- [x] Smooth camera animation when selecting parts
- [x] Click gizmo faces to snap to aligned view
- [x] Snap views: front, back, left, right, top, bottom, iso
- [x] Transform gizmo appears for single selection
- [x] Translate mode moves part along axes
- [x] Rotate mode rotates part around axes
- [x] Scale mode scales part uniformly or per-axis
- [x] Grid snap constrains transforms to increment
- [x] Wireframe mode toggles edge-only rendering
- [x] Double-click empty canvas resets camera
- [x] Expensive effects disabled during camera motion
- [x] Camera settings persist across sessions

## Future Enhancements

- [ ] Multiple viewports (quad view: front/top/right/perspective)
- [ ] Orthographic projection mode
- [ ] View cube widget (click corners for iso views)
- [ ] Exploded view for assemblies
- [ ] Section plane with live cut display
- [ ] Walk-through mode for architectural scale
- [ ] VR/AR viewport support
