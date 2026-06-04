/**
 * SchematicView: the full-canvas 2D schematic editor.
 *
 * Shown when the electronics workspace is in the "schematic" layout. It fills
 * the viewport with an opaque background so the 3D board behind it is hidden —
 * the schematic and board views are mutually exclusive and the toolbar (or Tab)
 * toggles between them. This replaces the old floating, dockable overlay that
 * sat on top of the 3D canvas.
 */

import { Suspense, lazy } from "react";
import { useTheme } from "@/hooks/useTheme";

const SchematicCanvas = lazy(() =>
  import("./SchematicCanvas").then((m) => ({ default: m.SchematicCanvas })),
);

export function SchematicView() {
  const { isDark } = useTheme();
  // Opaque fill so the 3D board (still mounted behind) is fully hidden.
  const bgColor = isDark ? "#0a0a0a" : "#ffffff";

  return (
    <div
      className="absolute inset-0 z-20 overflow-hidden"
      style={{ backgroundColor: bgColor }}
    >
      <Suspense fallback={null}>
        <SchematicCanvas />
      </Suspense>
    </div>
  );
}
