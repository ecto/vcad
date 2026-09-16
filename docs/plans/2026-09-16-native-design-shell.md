# Native spatial Design shell

Design uses a single compact glass model navigator, the existing RealityKit
canvas, and a stable contextual inspector. Add and Modify menus reuse the
existing model actions and their enablement; Sketch is available for editable
documents. Escape cancels armed placement. The document title menu supports
Save and Show in Finder for existing documents, with native window edited-state
metadata maintained separately.

The feature outline shares existing selection, visibility and hierarchy logic.
The inspector preserves live edits and document parameter controls; sandbox
radius accepts bounded numeric input. Measurements are disclosed on demand.
Camera orientation, reset, visibility and zebra analysis live on the canvas.
A quiet selection breadcrumb and the existing command field complete the shell.
Manufacture retains its machine rail and controls.

This implements the mockup's shell using native SwiftUI controls and existing
glass/material surfaces. It does not invent per-edge selection, tangent
propagation, geometry-anchored radius handles or transactional Apply/Cancel.
Those require additional kernel/editing support; parameters remain live.

Validation: native layout snapshots at 800 and 1240 points, window-presentation
and document metadata regression checks, and existing CNC tests. Cached snapshots
do not fully capture system-composited glass. No physical CNC was connected.
