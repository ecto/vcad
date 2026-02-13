---
name: export
description: Quick export — save the current CAD document to STL or GLB
---

Export a CAD document to a file.

If a document was recently created with `create_cad_document` in this conversation, use that IR document.

If the user provides a format argument (e.g., `/vcad:export stl` or `/vcad:export glb`), use that format. Otherwise default to STL.

Steps:
1. Call `export_cad` with the IR document and a filename based on the part name + format extension
2. Report the output file path and size
3. Mention that STL is for 3D printing and GLB is for visualization/sharing

If no document exists in the conversation, ask the user to create one first with `/vcad:new-part` or `create_cad_document`.
