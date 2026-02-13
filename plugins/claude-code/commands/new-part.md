---
name: new-part
description: Guided 3D part creation — describe what you want to build
---

Help the user create a 3D CAD part step by step.

1. Ask what they want to build (bracket, enclosure, gear, plate, custom shape, etc.)
2. Ask for key dimensions (or suggest reasonable defaults)
3. Build the part using `create_cad_document` with the appropriate part type:
   - Simple shapes → primitive-based (cube, cylinder, sphere, cone)
   - Custom profiles → extrude with polygon sketch
   - Round bodies → revolve
   - Tubes/springs → sweep
   - Transitions → loft
4. Run `inspect_cad` to verify dimensions and volume
5. Show the results and ask if they want to:
   - Modify the design (add holes, fillets, patterns, etc.)
   - Export to STL or GLB via `export_cad`
   - Open in the browser via `open_in_browser`

Keep dimensions in millimeters. Default to `"format": "compact"` for efficiency.

If the user isn't sure what they need, suggest common starting points:
- Mounting plate with holes
- L-bracket or T-bracket
- Enclosure / box with shell
- Flanged hub or bushing
- Custom extruded profile
