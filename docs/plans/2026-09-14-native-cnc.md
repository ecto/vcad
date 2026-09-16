# Native CNC workspace

## Target and scope

Anolex 4030 Ultra 2, Grbl_ESP32 1.3a build `20211103`, configured host
`192.168.2.226`. TCP port defaults to 23 and is editable. The hardware was off
during implementation. No hardware connection or physical motion was attempted.

The native app has a windowed CNC workspace with Setup, Toolpaths and Machine
modes, a job outline, a contextual inspector and persistent transport controls.
Desktop mode retains the compact floating CNC panel. This first version
supports single-tool rectangular facing, pocketing and outside profiles. Regions
are entered numerically; they are not extracted from selected CAD faces. It does
not yet expose arbitrary contours, tabs, 3D roughing, probing, tool changes,
fixture collision checks, machine travel verification or material removal simulation.
The displayed stock is a reference block, not a simulation of remaining material.
Operations can be added, reordered and removed; generation/export work for one
operation or the full job. A combined job is limited to one tool diameter and
contains one final program-end command.

## Architecture

- `CNCWorkspace` owns the editable setup, immutable generated setup/program,
  CAD placement and `CNCController`. Generation runs off the main actor through
  `vcad_cam_generate`; the caller frees its JSON result with `vcad_cam_free`.
- The Rust bridge validates finite dimensions and pass counts, invokes the
  existing CAM kernel, checks generated bounds, and emits explicit metric
  absolute G54 motion. Retracts are vertical before rapid XY travel. The
  single-tool post has no M6. It starts M3 at the requested RPM and ends M5/M2.
- Corrected native pocket/profile offsets: geo-clipper already applies its
  scaling factor to the offset distance; applying it twice generated an
  oversized profile and reduced pocketing to its centre fallback.
- The TCP transport uses Network.framework. Connection epochs reject callbacks
  from closed connections. A bounded byte framer handles fragmented/coalesced
  replies. Startup queries `$I`, `$$` and `$G` establish version, `$13` report
  units and active work coordinates without changing persistent settings.
- Status is polled at 5 Hz. `MPos`, `WPos`, intermittent `WCO`, feed and spindle
  values feed the native panel and existing RealityKit kernel-coordinate frame.
  Inch reports are converted to millimetres; a missing offset does not imply zero.
  G54 must be active for the viewport's work-position marker and starting a job.
- One G-code line is outstanding at a time. Acknowledgements count accepted
  lines. The final acknowledgement enters a draining state; a subsequent Idle
  report completes the job. This conservative sender may not saturate the
  planner on very short segments.
- Feed hold, resume, raw jog cancel, soft reset, homing, G54 selection and XYZ
  work zero are explicit actions. Errors, controller restarts, lost telemetry
  and disconnects stop streaming. Jobs never resume automatically after faults.
  Resume waits for a controller report that the hold has cleared.
- Planned cuts are orange, rapids cyan, reported tool position green. A separate
  translucent orange tool follows the selected generated path during preview.
  Scrubbing and timed playback never send controller commands. Cutting durations
  use per-move CAM feeds; rapid time assumes 3000 mm/min and is labelled estimated.
  Acceleration, initial approach and spindle run-up time are not included. The CAD
  origin field maps G54 zero into the model; it does not change machine zero.
  Geometry is batched into two path meshes; telemetry only moves the tool marker.
  A stale position is hidden. Live position can be shown without generating a job.

## Use

1. Open **CNC** in the bottom bar. With the machine off, choose **Simulator**.
2. Set operation, stock rectangle, cutter, cutting depth, feeds, RPM and clearance.
   XY zero is the rectangle's lower-left; Z zero is the stock top.
3. Generate and inspect the path; export `.nc` if desired. Editing the setup
   invalidates Run until regeneration. The preset feeds/RPM are examples, not
   material-specific cutting recommendations.
4. When hardware is available, connect, verify telemetry, home as appropriate,
   establish G54 zero, and place that origin in CAD. Only one sender should
   control the machine; changes made by another client are not synchronized.
5. Review tool, workholding, stock, clearance and origin, then use Run. Homing,
   zeroing and starting a job each have a concrete confirmation in the app.

A network hold/reset is not a hardware E-stop. A failed connection cannot
establish that a hold reached the machine. Profile paths have no holding tabs;
workholding must support the operation independently.

## Validation and development

- `cargo test -p vcad-kernel-cam --lib`
- `cargo test -p vcad-ffi cam::tests --lib`
- Build/stage the library using `apple/VcadApp/build-ffi.sh`, then
  `swift test --package-path apple/VcadApp --filter CNCTests`.
- `apple/VcadApp/bundle.sh` produces the macOS app. The bundle declares local
  network usage. `VCAD_CNC_DEMO=1` opens the CNC panel with a generated facing
  job and an offline simulator; this hook never connects to hardware.

Tests cover geometric bounds and vertical retracts, invalid inputs, byte
framing, coordinate offsets and report units, acknowledgement sequencing,
final-Idle completion, simulator hold/resume, RealityKit entities, and a real
loopback TCP peer including errors, stale telemetry and disconnects. Hardware
validation of this particular firmware build, spindle wiring and travel limits
remains outstanding.

Protocol references: [Grbl_ESP32](https://github.com/bdring/Grbl_Esp32) and
[controller commands](https://github.com/bdring/Grbl_Esp32/blob/main/doc/Commands.txt).

## Window presentation

View → Open in Window / Release to Desktop (⇧⌘W) changes the existing AppKit
window in place. The same renderer, camera, selection and CNC session survive;
the last windowed frame is restored on return. Opening the CNC panel selects
windowed mode. `VCAD_WINDOWED=1` launches directly into a normal resizable window.
The windowed CNC workspace implements the three-mode layout from the mockup.
The central ARView stays in the same SwiftUI layout slot across presentation
changes. Use Design to return to the CAD panels, or Release to Desktop to keep
working with floating panels. Opening CNC again returns to the unified window.

## Workspace validation

`CNCStudioTests` covers generated-motion interpolation, per-move feeds, shared
stock/tool invalidation, operation ordering, single-end combined G-code,
preview/report separation and run gating. Existing controller loopback and
window-transition tests remain applicable. The inspector's Use model bounds
sets rectangular stock dimensions and a top-corner origin; it does not infer
machinable faces or set a physical machine work offset.

### Dojo panel layout

The windowed CNC workspace follows `ipse/apple/Dojo/Sources/Dojo/App.swift`:
an uninterrupted viewport with three inset, rounded panels and 12-point gutters.
The left panel owns job structure, the right owns machine telemetry and controls,
and the full-width bottom panel inspects the current selection in horizontal groups.
Toolbar buttons independently hide each panel. Transport and job controls remain
visible above the inspector even when its parameter area is hidden. Side panels
scroll within the space above the bottom panel; controller and selection state
remain shared with desktop presentation.

Panel appearance now uses SwiftUI Liquid Glass on macOS 26 and native ultra-thin
material on older systems. Transport and footer inherit the panel surface; they
no longer paint opaque strips. Controls use the system accent and native text
fields. Orange/cyan in the viewport continue to identify cutting/rapid motion.

### Unified workspace header

Design and Manufacture share `WorkspaceHeader`: document identity, workspace
selector, controller status, contextual panel toggles, and presentation toggle.
Normal windows host it on a native toolbar material; desktop presentation hosts
it in an inset Liquid Glass surface. Manufacturing stages live in the job panel.
Both presentations retain the three CNC panels and use the same workspace state.
