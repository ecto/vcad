/**
 * SchematicOverlayPanel: CSS-resizable div overlay containing the existing SchematicCanvas.
 *
 * Docks to the left or right of the viewport. Collapsible via toggle button.
 * Uses CSS `resize: horizontal` for free browser drag-to-resize.
 */

import { Suspense, lazy } from "react";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useTheme } from "@/hooks/useTheme";

const SchematicCanvas = lazy(() =>
  import("./SchematicCanvas").then((m) => ({ default: m.SchematicCanvas })),
);

export function SchematicOverlayPanel() {
  const schematicDocked = useElectronicsStore((s) => s.schematicDocked);
  const { isDark } = useTheme();

  if (schematicDocked === "hidden") return null;

  const isLeft = schematicDocked === "left";
  const bgColor = isDark ? "rgba(10, 10, 10, 0.95)" : "rgba(255, 255, 255, 0.95)";
  const borderColor = isDark ? "#333" : "#ddd";

  return (
    <div
      className="absolute top-0 bottom-0 z-20 flex flex-col"
      style={{
        [isLeft ? "left" : "right"]: 0,
        width: 400,
        minWidth: 250,
        maxWidth: "60%",
        resize: "horizontal",
        overflow: "hidden",
        backgroundColor: bgColor,
        borderRight: isLeft ? `1px solid ${borderColor}` : undefined,
        borderLeft: !isLeft ? `1px solid ${borderColor}` : undefined,
        backdropFilter: "blur(8px)",
      }}
      onClick={() => useElectronicsStore.setState({ focusedPane: "schematic" })}
    >
      {/* Header */}
      <div
        className="flex items-center justify-between px-2 h-7 shrink-0 select-none"
        style={{
          backgroundColor: isDark ? "rgba(30, 30, 30, 0.9)" : "rgba(240, 240, 240, 0.9)",
          borderBottom: `1px solid ${borderColor}`,
        }}
      >
        <span className="text-[10px] font-medium text-text-muted uppercase tracking-wider">
          Schematic
        </span>
        <button
          onClick={(e) => {
            e.stopPropagation();
            useElectronicsStore.setState({ schematicDocked: "hidden" });
          }}
          className="text-[10px] text-text-muted hover:text-text px-1"
          title="Hide schematic panel"
        >
          Hide
        </button>
      </div>

      {/* Schematic canvas */}
      <div className="flex-1 overflow-hidden relative">
        <Suspense fallback={null}>
          <SchematicCanvas />
        </Suspense>
      </div>
    </div>
  );
}
