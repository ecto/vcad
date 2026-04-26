/**
 * Bottom toolbar for electronics mode.
 * Mirrors the pattern of SketchToolbar: fixed bottom-center, uses
 * TabDropdown + ToolbarButton from the shared toolbar primitives.
 *
 * Tabs: Schematic | Components | PCB | View | Finish
 */

import { useEffect, useCallback } from "react";
import { Cursor } from "@phosphor-icons/react/dist/ssr/Cursor";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { Plugs } from "@phosphor-icons/react/dist/ssr/Plugs";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Cpu } from "@phosphor-icons/react/dist/ssr/Cpu";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Tag } from "@phosphor-icons/react/dist/ssr/Tag";
import { Path } from "@phosphor-icons/react/dist/ssr/Path";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Columns } from "@phosphor-icons/react/dist/ssr/Columns";
import { Square } from "@phosphor-icons/react/dist/ssr/Square";
import { MagnetStraight } from "@phosphor-icons/react/dist/ssr/MagnetStraight";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { Ruler } from "@phosphor-icons/react/dist/ssr/Ruler";
import { ArrowSquareDown } from "@phosphor-icons/react/dist/ssr/ArrowSquareDown";
import { TabDropdown, ToolbarButton } from "@/components/ui/toolbar";
import {
  ELECTRONICS_TAB_COLORS,
  type ElectronicsTab,
} from "@/components/ui/toolbar-constants";
import { cn } from "@/lib/utils";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { useUiStore } from "@vcad/core";
import type { PcbLayer } from "@vcad/ir";

import { SYMBOL_LIBRARY } from "./symbol-library";

// ---------------------------------------------------------------------------
// Tab definitions
// ---------------------------------------------------------------------------

const ELECTRONICS_TABS: {
  id: ElectronicsTab;
  label: string;
  icon: React.ComponentType<{ size?: number; weight?: "regular" | "fill"; className?: string }>;
}[] = [
  { id: "schematic", label: "Schematic", icon: PencilSimple },
  { id: "components", label: "Components", icon: Package },
  { id: "pcb", label: "PCB", icon: Cpu },
  { id: "view", label: "View", icon: Eye },
  { id: "finish", label: "Finish", icon: X },
];

// ---------------------------------------------------------------------------
// Schematic symbol icons (16x16 SVGs)
// ---------------------------------------------------------------------------

function SymbolIcon({ id }: { id: string }) {
  const s = 16; // viewBox size
  const common = { width: s, height: s, viewBox: `0 0 ${s} ${s}`, fill: "none", stroke: "currentColor", strokeWidth: 1.5, strokeLinecap: "round" as const, strokeLinejoin: "round" as const };

  switch (id) {
    case "resistor": // IEC rectangle
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="4" y2="8" />
          <rect x="4" y="5" width="8" height="6" rx="0.5" />
          <line x1="12" y1="8" x2="15" y2="8" />
        </svg>
      );
    case "capacitor": // two parallel lines
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="6" y2="8" />
          <line x1="6" y1="3" x2="6" y2="13" />
          <line x1="10" y1="3" x2="10" y2="13" />
          <line x1="10" y1="8" x2="15" y2="8" />
        </svg>
      );
    case "led": // diode triangle + arrows
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="5" y2="8" />
          <polygon points="5,4 5,12 11,8" fill="currentColor" opacity="0.3" stroke="currentColor" />
          <line x1="11" y1="4" x2="11" y2="12" />
          <line x1="11" y1="8" x2="15" y2="8" />
          <line x1="9" y1="2" x2="12" y2="0" />
          <line x1="11" y1="3" x2="14" y2="1" />
        </svg>
      );
    case "diode": // triangle + bar
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="5" y2="8" />
          <polygon points="5,4 5,12 11,8" fill="currentColor" opacity="0.3" stroke="currentColor" />
          <line x1="11" y1="4" x2="11" y2="12" />
          <line x1="11" y1="8" x2="15" y2="8" />
        </svg>
      );
    case "npn": // transistor
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="5" y2="8" />
          <line x1="5" y1="4" x2="5" y2="12" />
          <line x1="5" y1="6" x2="12" y2="2" />
          <line x1="5" y1="10" x2="12" y2="14" />
          <circle cx="8" cy="8" r="6" strokeWidth="1" opacity="0.4" />
        </svg>
      );
    case "ic8": // chip rectangle with pins
      return (
        <svg {...common}>
          <rect x="4" y="2" width="8" height="12" rx="0.5" />
          <line x1="1" y1="5" x2="4" y2="5" />
          <line x1="1" y1="8" x2="4" y2="8" />
          <line x1="1" y1="11" x2="4" y2="11" />
          <line x1="12" y1="5" x2="15" y2="5" />
          <line x1="12" y1="8" x2="15" y2="8" />
          <line x1="12" y1="11" x2="15" y2="11" />
          <circle cx="6" cy="4" r="0.8" fill="currentColor" stroke="none" />
        </svg>
      );
    case "vcc": // power up arrow
      return (
        <svg {...common}>
          <line x1="8" y1="14" x2="8" y2="5" />
          <polyline points="4,5 8,1 12,5" />
        </svg>
      );
    case "gnd": // ground symbol
      return (
        <svg {...common}>
          <line x1="8" y1="2" x2="8" y2="7" />
          <line x1="3" y1="7" x2="13" y2="7" />
          <line x1="5" y1="10" x2="11" y2="10" />
          <line x1="7" y1="13" x2="9" y2="13" />
        </svg>
      );
    default:
      return <span className="text-xs font-bold">{id[0]?.toUpperCase()}</span>;
  }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function ElectronicsToolbar() {
  const focusedPane = useElectronicsStore((s) => s.focusedPane);
  const schTool = useElectronicsStore((s) => s.schTool);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const layout = useElectronicsStore((s) => s.layout);
  const placingSymbol = useElectronicsStore((s) => s.schPlacingSymbol);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const pcbGridSize = useElectronicsStore((s) => s.pcbGridSize);
  const pcbSnapToGrid = useElectronicsStore((s) => s.pcbSnapToGrid);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const schLabelName = useElectronicsStore((s) => s.schLabelName);
  const selection = useElectronicsStore((s) => s.selection);

  const setSchTool = useElectronicsStore((s) => s.setSchTool);
  const setPcbTool = useElectronicsStore((s) => s.setPcbTool);
  const setLayout = useElectronicsStore((s) => s.setLayout);
  const setSchPlacingSymbol = useElectronicsStore((s) => s.setSchPlacingSymbol);
  const rotateSchPlacement = useElectronicsStore((s) => s.rotateSchPlacement);
  const cancelSchWire = useElectronicsStore((s) => s.cancelSchWire);
  const setPcbActiveLayer = useElectronicsStore((s) => s.setPcbActiveLayer);
  const setPcbGridSize = useElectronicsStore((s) => s.setPcbGridSize);
  const setPcbSnapToGrid = useElectronicsStore((s) => s.setPcbSnapToGrid);
  const setLayerVisible = useElectronicsStore((s) => s.setLayerVisible);
  const setSchLabelName = useElectronicsStore((s) => s.setSchLabelName);
  const exit = useElectronicsStore((s) => s.exit);

  const unplacedComponents = useElectronicsStore((s) => s.unplacedComponents);
  const syncSchematicToPcb = useDocumentStore((s) => s.syncSchematicToPcb);

  const removeTrace = useDocumentStore((s) => s.removeTrace);
  const removeVia = useDocumentStore((s) => s.removeVia);
  const rotateFootprint = useDocumentStore((s) => s.rotateFootprint);
  const flipFootprint = useDocumentStore((s) => s.flipFootprint);

  const isOrbiting = useUiStore((s) => s.isOrbiting);

  // ---------------------------------------------------------------------------
  // Place a component: sets focused pane, tool, and symbol atomically
  // ---------------------------------------------------------------------------

  const placeComponent = useCallback(
    (symbolId: string) => {
      useElectronicsStore.setState({
        focusedPane: "schematic",
        schTool: "place",
        schPlacingSymbol: symbolId,
        schPlacingRotation: 0,
      });
    },
    [],
  );

  // ---------------------------------------------------------------------------
  // Keyboard shortcuts
  // ---------------------------------------------------------------------------

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).tagName === "INPUT") return;
      const key = e.key;

      switch (key) {
        // Layout
        case "1":
          setLayout("split");
          break;
        case "2":
          setLayout("schematic-only");
          break;
        case "3":
          setLayout("pcb-only");
          break;

        // Escape: cascading cancel
        case "Escape": {
          const store = useElectronicsStore.getState();
          if (store.schWireStart) {
            cancelSchWire();
          } else if (store.schPlacingSymbol) {
            setSchPlacingSymbol(null);
          } else if (store.schTool !== "select" || store.pcbTool !== "select") {
            setSchTool("select");
            setPcbTool("select");
          } else {
            exit();
          }
          break;
        }

        // Delete selection
        case "Delete":
        case "Backspace": {
          const sel = useElectronicsStore.getState().selection;
          const boardNodeId = useCoreElectronicsStore.getState().activeBoardNodeId;
          if (sel.type === "trace" && boardNodeId != null) {
            removeTrace(boardNodeId, sel.idx);
            useElectronicsStore.getState().select({ type: "none" });
          } else if (sel.type === "via" && boardNodeId != null) {
            removeVia(boardNodeId, sel.idx);
            useElectronicsStore.getState().select({ type: "none" });
          } else if (sel.type === "footprint" && boardNodeId != null) {
            const doc = useDocumentStore.getState().document;
            const pcb = getNodePcb(doc, boardNodeId);
            if (pcb) {
              const idx = pcb.footprints.findIndex((f) => f.ref === sel.ref);
              if (idx >= 0) {
                useDocumentStore.getState().removeFootprint(boardNodeId, idx);
                const sch = doc.schematic;
                if (sch) {
                  const ci = sch.components.findIndex((c) => c.ref === sel.ref);
                  if (ci >= 0) useDocumentStore.getState().removeSchematicComponent(ci, boardNodeId);
                }
              }
            }
            useElectronicsStore.getState().select({ type: "none" });
          } else if (sel.type === "component") {
            const sch = useDocumentStore.getState().document.schematic;
            if (sch) {
              const ci = sch.components.findIndex((c) => c.ref === sel.ref);
              if (ci >= 0) useDocumentStore.getState().removeSchematicComponent(ci, boardNodeId ?? undefined);
            }
            useElectronicsStore.getState().select({ type: "none" });
          }
          break;
        }

        // Schematic tools
        case "w":
        case "W":
          if (focusedPane === "schematic") setSchTool("wire");
          break;
        case "l":
        case "L":
          if (focusedPane === "schematic") setSchTool("label");
          break;
        case "d":
        case "D":
          if (focusedPane === "schematic") setSchTool("delete");
          else if (focusedPane === "pcb") setPcbTool("delete");
          break;

        // Shared select/move
        case "v":
        case "V":
          if (focusedPane === "pcb") setPcbTool("select");
          else setSchTool("select");
          break;
        case "m":
        case "M":
          if (focusedPane === "pcb") setPcbTool("move");
          else setSchTool("move");
          break;
        case "x":
        case "X":
          if (focusedPane === "pcb") setPcbTool("route");
          break;
        case "t":
        case "T":
          if (focusedPane === "pcb") setPcbTool("length-tune");
          break;

        // Rotate / flip
        case "r":
        case "R":
          if (focusedPane === "schematic" && schTool === "place") {
            rotateSchPlacement();
          } else if (focusedPane === "pcb" && selection.type === "footprint") {
            const bNodeId = useCoreElectronicsStore.getState().activeBoardNodeId;
            if (bNodeId != null) {
              const pcb = getNodePcb(useDocumentStore.getState().document, bNodeId);
              if (pcb) {
                const idx = pcb.footprints.findIndex((f) => f.ref === selection.ref);
                if (idx >= 0) rotateFootprint(bNodeId, idx, 90);
              }
            }
          }
          break;
        case "f":
        case "F":
          if (focusedPane === "pcb" && selection.type === "footprint") {
            const bNodeId = useCoreElectronicsStore.getState().activeBoardNodeId;
            if (bNodeId != null) {
              const pcb = getNodePcb(useDocumentStore.getState().document, bNodeId);
              if (pcb) {
                const idx = pcb.footprints.findIndex((f) => f.ref === selection.ref);
                if (idx >= 0) flipFootprint(bNodeId, idx);
              }
            }
          } else if (focusedPane === "pcb") {
            setPcbActiveLayer("FCu");
          }
          break;
        case "b":
        case "B":
          if (focusedPane === "pcb") setPcbActiveLayer("BCu");
          break;
      }
    },
    [
      focusedPane, schTool, selection,
      setLayout, exit, setSchTool, setPcbTool, setPcbActiveLayer,
      cancelSchWire, setSchPlacingSymbol, rotateSchPlacement,
      removeTrace, removeVia, rotateFootprint, flipFootprint,
    ],
  );

  useEffect(() => {
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleKeyDown]);

  // ---------------------------------------------------------------------------
  // Tab content renderers
  // ---------------------------------------------------------------------------

  const renderSchematicContent = () => (
    <>
      <ToolbarButton
        tooltip="Select (V)"
        active={schTool === "select"}
        onClick={() => setSchTool("select")}
        iconColor={ELECTRONICS_TAB_COLORS.schematic}
      >
        <Cursor size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Move (M)"
        active={schTool === "move"}
        onClick={() => setSchTool("move")}
        iconColor={ELECTRONICS_TAB_COLORS.schematic}
      >
        <ArrowsOutCardinal size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Wire (W)"
        active={schTool === "wire"}
        onClick={() => setSchTool("wire")}
        iconColor={ELECTRONICS_TAB_COLORS.schematic}
      >
        <Path size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Label (L)"
        active={schTool === "label"}
        onClick={() => setSchTool("label")}
        iconColor={ELECTRONICS_TAB_COLORS.schematic}
      >
        <Tag size={20} />
      </ToolbarButton>
      {schTool === "label" && (
        <input
          type="text"
          value={schLabelName}
          onChange={(e) => setSchLabelName(e.target.value)}
          className="w-20 text-[11px] bg-transparent text-text border border-border rounded px-1 py-0.5"
          placeholder="Label name"
          onClick={(e) => e.stopPropagation()}
        />
      )}
      <ToolbarButton
        tooltip="Delete (D)"
        active={schTool === "delete"}
        onClick={() => setSchTool("delete")}
        iconColor={ELECTRONICS_TAB_COLORS.schematic}
      >
        <Trash size={20} />
      </ToolbarButton>
    </>
  );

  const renderComponentsContent = () => (
    <>
      {SYMBOL_LIBRARY.map((sym) => (
        <ToolbarButton
          key={sym.id}
          tooltip={`${sym.name} (${sym.defaultValue})`}
          active={placingSymbol === sym.id}
          onClick={() => placeComponent(sym.id)}
          iconColor={ELECTRONICS_TAB_COLORS.components}
        >
          <SymbolIcon id={sym.id} />
        </ToolbarButton>
      ))}
    </>
  );

  const renderPcbContent = () => (
    <>
      <ToolbarButton
        tooltip="Select (V)"
        active={pcbTool === "select"}
        onClick={() => setPcbTool("select")}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <Cursor size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Move (M)"
        active={pcbTool === "move"}
        onClick={() => setPcbTool("move")}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <ArrowsOutCardinal size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Route trace (X)"
        active={pcbTool === "route"}
        onClick={() => setPcbTool("route")}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <Plugs size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Length tune (T)"
        active={pcbTool === "length-tune"}
        onClick={() => setPcbTool("length-tune")}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <Ruler size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Delete (D)"
        active={pcbTool === "delete"}
        onClick={() => setPcbTool("delete")}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <Trash size={20} />
      </ToolbarButton>
      {unplacedComponents.length > 0 && (
        <ToolbarButton
          tooltip={`Place ${unplacedComponents.length} unplaced component(s)`}
          onClick={() => {
            const boardNodeId = useCoreElectronicsStore.getState().activeBoardNodeId;
            if (boardNodeId != null) syncSchematicToPcb(boardNodeId);
          }}
          iconColor={ELECTRONICS_TAB_COLORS.pcb}
        >
          <ArrowSquareDown size={20} />
        </ToolbarButton>
      )}
    </>
  );

  const renderViewContent = () => (
    <>
      {/* Layout toggles */}
      <ToolbarButton
        tooltip="Split view (1)"
        active={layout === "split"}
        onClick={() => setLayout("split")}
        iconColor={ELECTRONICS_TAB_COLORS.view}
      >
        <Columns size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="Schematic only (2)"
        active={layout === "schematic-only"}
        onClick={() => setLayout("schematic-only")}
        iconColor={ELECTRONICS_TAB_COLORS.view}
        label="Sch"
        expanded
      >
        <Square size={20} />
      </ToolbarButton>
      <ToolbarButton
        tooltip="PCB only (3)"
        active={layout === "pcb-only"}
        onClick={() => setLayout("pcb-only")}
        iconColor={ELECTRONICS_TAB_COLORS.view}
        label="PCB"
        expanded
      >
        <Square size={20} />
      </ToolbarButton>

      <div className="w-px h-5 bg-border mx-0.5" />

      {/* Grid / Snap */}
      <div className="flex items-center gap-1 px-1">
        <span className="text-[10px] text-text-muted">Grid:</span>
        <select
          className="text-[10px] bg-transparent text-text border border-border rounded px-1 py-0.5"
          value={pcbGridSize}
          onChange={(e) => setPcbGridSize(Number(e.target.value))}
          onClick={(e) => e.stopPropagation()}
        >
          <option value={0.1}>0.1mm</option>
          <option value={0.25}>0.25mm</option>
          <option value={0.5}>0.5mm</option>
          <option value={1.0}>1mm</option>
          <option value={2.54}>2.54mm</option>
        </select>
      </div>
      <ToolbarButton
        tooltip="Toggle snap to grid"
        active={pcbSnapToGrid}
        onClick={() => setPcbSnapToGrid(!pcbSnapToGrid)}
        iconColor={pcbSnapToGrid ? "text-green-400" : undefined}
      >
        <MagnetStraight size={18} />
      </ToolbarButton>

      <div className="w-px h-5 bg-border mx-0.5" />

      {/* Layer selector */}
      <div className="flex items-center gap-1 px-1">
        <span className="text-[10px] text-text-muted">Layer:</span>
        <select
          className="text-[10px] bg-transparent text-text border border-border rounded px-1 py-0.5"
          value={pcbActiveLayer}
          onChange={(e) => setPcbActiveLayer(e.target.value as PcbLayer)}
          onClick={(e) => e.stopPropagation()}
        >
          {pcbLayers
            .filter((l) => l.layer.endsWith("Cu"))
            .map((l) => (
              <option key={l.layer} value={l.layer}>
                {l.layer}
              </option>
            ))}
        </select>
      </div>

      {/* Layer visibility toggles (copper only) */}
      {pcbLayers
        .filter((l) => l.layer.endsWith("Cu"))
        .map((l) => (
          <ToolbarButton
            key={l.layer}
            tooltip={`${l.layer} ${l.visible ? "(visible)" : "(hidden)"}`}
            active={l.visible}
            onClick={() => setLayerVisible(l.layer, !l.visible)}
            iconColor={l.visible ? undefined : "opacity-30"}
          >
            <span
              className="w-3 h-3 rounded-full border border-border"
              style={{ backgroundColor: l.color }}
            />
          </ToolbarButton>
        ))}

    </>
  );

  const renderFinishContent = () => (
    <ToolbarButton
      tooltip="Exit electronics (Esc)"
      onClick={exit}
      iconColor={ELECTRONICS_TAB_COLORS.finish}
    >
      <X size={20} />
    </ToolbarButton>
  );

  return (
    <div
      className={cn(
        "fixed left-1/2 bottom-4 sm:bottom-6 z-30 -translate-x-1/2",
        "max-w-[calc(100vw-16px)] sm:max-w-none",
        "transition-opacity duration-200",
        isOrbiting && "opacity-0 pointer-events-none",
      )}
    >
      <div
        className={cn(
          "relative flex items-center gap-0.5",
          "bg-surface/95 backdrop-blur-sm",
          "overflow-x-auto scrollbar-none",
        )}
      >
        {ELECTRONICS_TABS.map(({ id, label, icon }) => (
          <TabDropdown
            key={id}
            id={id}
            label={label}
            icon={icon}
            colors={ELECTRONICS_TAB_COLORS}
          >
            {id === "schematic" && renderSchematicContent()}
            {id === "components" && renderComponentsContent()}
            {id === "pcb" && renderPcbContent()}
            {id === "view" && renderViewContent()}
            {id === "finish" && renderFinishContent()}
          </TabDropdown>
        ))}
      </div>
    </div>
  );
}

export default ElectronicsToolbar;
