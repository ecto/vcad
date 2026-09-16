# Native Manufacture machine tools

Machine controls extend the existing Manufacture workspace, with its Setup /
Toolpaths / Machine workflow, native SwiftUI inspectors and RealityKit viewport.
There is one controller session and no separate console screen or theme.

## Workspace integration

- The outline imports G-code and switches between imported and generated jobs.
- The existing RealityKit viewport displays the selected job, with top, side and
  3D views, auto-fit and optional spindle-follow. Imported jobs hide assumed stock.
- The machine inspector contains work/machine coordinates, jogging, work offsets,
  overrides, accessories and probing, using native controls and disclosure groups.
- The lower inspector adds Terminal, G-code and Macros tabs. Transport and run
  gates operate on the selected job source.
- Incremental diagonal/XYZ jogs, raw jog cancel, home, per-axis/XY zero,
  return-to-zero and a session-local saved machine park position.
- G54–G59 selection for manual work. Generated and imported jobs require G54.
- GRBL-reported feed/spindle overrides. Slider changes send one realtime byte at
  a time and wait for feedback, since GRBL's realtime flags do not queue repeated
  bytes. Unknown overrides remain unavailable, not assumed to be 100 percent.
- Idle coolant commands (M7/M8/M9), spindle stop, cycle/hold/resume/reset,
  accepted-line progress and elapsed wall time. Acceptance is not cutting progress.
- Terminal with MDI, copy, clear and auto-scroll; G-code view; persisted named
  single-line macros with review before execution; manually installed tool info.
- Z touch-off: bounded downward G38.2, successful PRB capture, explicit plate
  thickness application in millimetres. Historical PRB queries cannot authorize
  zeroing. New manual commands invalidate previous probe results.

## Program import

Imports .nc / .gcode / plain text up to 8 MB, rejecting realtime characters,
controller settings, oversized lines and early program-end blocks. Supports
G0/G1, XY G2/G3 with I/J or R (including helical Z), G20/G21, G90/G91, G54,
G17/G94, G4 dwell and neutral G40/G49/G80. Spindle/coolant and program-end M
codes are supported. Other coordinate frames, machine moves, canned cycles,
multiple tools and automatic tool changes are rejected rather than mispreviewed.
A known G21 G90 G17 G54 G94 F400 preamble matches preview defaults. Initial
position is XYZ0 for preview; it is labelled as an assumption, not a collision
or initial-travel check. The preview tessellation is capped at 250,000 moves.

## Controller behavior

Uses the existing Anolex TCP connection and simulator. This does not
introduce a second controller owner. Manual sequences and modal-state refreshes
are acknowledgement-serialized; manual motion cannot interleave with jobs.
A final G4 planner barrier precedes job completion; fault/stale-telemetry handling
still requests hold and prevents automatic resume. G-code cannot change report
units through MDI. Work offsets are invalidated after potentially changing
commands until the controller reports them again. Simulator work offsets,
probing, coolant and overrides are separate from physical hardware.

## Verification

`VCAD_CNC_SNAPSHOTS=1 swift test --package-path apple/VcadApp --filter CNC`
exercises protocol parsing, existing CAM and fault handling, simulator controls,
imported arcs/units, Manufacture job selection and a loopback TCP GRBL peer.
Optional panel snapshots write to `/tmp/vcad-manufacture/`; these do not capture
the RealityKit viewport. `VCAD_CNC_DEMO=1 VCAD_WINDOWED=1` opens Manufacture
with the simulator through the existing offline launch hook.

No physical mill was connected during development. Coolant, probing and spindle
behavior depend on installed hardware and firmware. Tool changers/TLS are not
configured. Imported paths have no stock-removal or fixture-collision simulation.

## Persistent machine rail

The bottom surface now keeps physical controls visible in every Manufacture
stage. It uses native SwiftUI buttons, menus, sliders, popovers and dialogs,
with the existing macOS glass material (material fallback on older macOS).
Work coordinates are primary; machine coordinates are explicitly secondary.
Disconnected or stale values display a dash. Work-offset selection and motion
controls honor the controller's command gates.

Jog opens a native popover, Zero a per-axis menu with confirmation, and Probe
opens the existing touch-off sheet. Feed override uses controller feedback;
unknown values never imply a 100% report. Spindle override, coolant, park and
homing remain available in the reduced machine inspector.

Run/Hold/Resume and Stop occupy fixed slots. Stop immediately requests feed hold,
then offers a destructive controller-reset confirmation. Cancelling that dialog
leaves the hold request in effect. Stop is not a physical emergency stop.
The rail reports accepted line counts rather than a cutting-progress percentage.
Review setup opens readiness and the operator confirmation in a native popover.
Terminal toggles the drawer above the rail; G-code, macros and export remain in
the adjacent Job tools menu. Simulation has its own clearly labeled strip.

The optional snapshot test covers minimum-width and wide rails in native light
and dark appearances. Cached view snapshots validate layout but cannot fully
capture system-composited glass and button materials.
