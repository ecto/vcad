# Native Electronics workspace

The workspace menu now includes Design, Electronics and Manufacture. Design
commands occupy a separate native command bar with a tool-group picker. The
window header stays document-focused. Native traffic lights align to its 52pt
height on windowed presentation and resize.

Electronics edits the same document JSON used by the mechanical kernel. It
reads PcbBoard nodes (including multiple boards) and the legacy pcb field;
new boards use PcbBoard nodes with real IDs and scene roots. Schematic components
and explicit nets use the shared IR. Unrecognized fields are preserved.

Implemented native workflow:
- Create a two-layer board; open and save vcad documents.
- Place starter resistor, capacitor and connector instances with explicit generic
  two-pad SMD geometry in both schematic and PCB data.
- Select and drag components; edit position, rotation and shared value.
- Connect two schematic pins through explicit nets; merge net assignments and
  keep board pad/copper net references synchronized.
- Route straight copper segments on FCu/BCu; place and delete vias/traces.
- Choose grid, snapping, layer, net, width and zoom.
- Orbit a SceneKit 3D preview; board outline and cutouts are geometric, component
  bodies/pads are simplified preview shapes.
- Run limited connectivity/metadata checks with their scope stated in the UI.
- Undo/redo compound board/schematic edits atomically.

This is an initial native editor, not web feature parity. Copper zones, trace
arcs and imported symbol/custom graphics are preserved but not rendered. Full
clearance DRC/ERC, autorouting, length tuning, circuit simulation/analysis,
full component/footprint libraries, net labels, fabrication exports and PCB
constraint tools remain to be ported. Starter footprints must be replaced or
verified for physical parts. 3D preview does not assert assembly clearance.

Electronics uses the same inset floating glass panels as Manufacture, with
searchable Parts/Nets/Layers navigation and native segmented tool controls.
The routing inspector includes a linked net-membership inset; pointer previews
show proposed straight traces. Starting from a pad inherits its net, rejects
endpoints on a different net, and preserves exact off-grid pad coordinates.
The preview is not clearance validation or a full schematic-symbol renderer.

Validation: 34 selected Swift workspace/CNC tests passed, including kernel
deserialization, save/reopen, atomic undo, routing gates, off-grid pad coordinates,
and window/layout checks. Rust library suites passed 55 FFI tests (1 ignored)
and 66 CAM tests. The bundled app built and the Electronics glass panes were
visually checked in the running app. No physical CNC machine was used.
