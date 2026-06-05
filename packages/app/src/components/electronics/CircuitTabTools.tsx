/**
 * Circuit-tab tool row for the main ToolPalette.
 *
 * Replaces the old bottom `ElectronicsToolbar`: when the Circuit tab is the
 * active palette tab this renders the electronics tools inline in the palette's
 * tool row, context-aware by the schematic/board view (the top-center toggle).
 * When no circuit is being edited it offers the entry point — "Edit Circuit"
 * if a board exists, otherwise "New PCB Board".
 *
 * The Circuit tab ⟺ electronics-active coupling lives in ToolPalette
 * (handleTabClick + autoSwitchTab); this component just renders tools.
 */

import { useCallback } from "react";
import { Cursor } from "@phosphor-icons/react/dist/ssr/Cursor";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { Plugs } from "@phosphor-icons/react/dist/ssr/Plugs";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Cpu } from "@phosphor-icons/react/dist/ssr/Cpu";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Tag } from "@phosphor-icons/react/dist/ssr/Tag";
import { Path } from "@phosphor-icons/react/dist/ssr/Path";
import { MagnetStraight } from "@phosphor-icons/react/dist/ssr/MagnetStraight";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Ruler } from "@phosphor-icons/react/dist/ssr/Ruler";
import { ArrowSquareDown } from "@phosphor-icons/react/dist/ssr/ArrowSquareDown";
import { Lightning } from "@phosphor-icons/react/dist/ssr/Lightning";
import { ToolbarButton } from "@/components/ui/toolbar";
import { ELECTRONICS_TAB_COLORS } from "@/components/ui/toolbar-constants";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useDocumentStore, useCoreElectronicsStore } from "@vcad/core";
import type { PcbLayer } from "@vcad/ir";
import { useSymbolLibrary } from "./symbol-library";
import { SymbolIcon } from "./symbol-icons";
import { autorouteRatsnest } from "@/lib/pcb-autoroute";

const Divider = () => <div className="mx-1 h-5 w-px bg-border shrink-0" />;

export function CircuitTabTools() {
  const active = useElectronicsStore((s) => s.active);
  const layout = useElectronicsStore((s) => s.layout);
  const symbols = useSymbolLibrary();
  const schTool = useElectronicsStore((s) => s.schTool);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const placingSymbol = useElectronicsStore((s) => s.schPlacingSymbol);
  const schLabelName = useElectronicsStore((s) => s.schLabelName);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const pcbGridSize = useElectronicsStore((s) => s.pcbGridSize);
  const pcbSnapToGrid = useElectronicsStore((s) => s.pcbSnapToGrid);
  const showComponentBodies = useElectronicsStore((s) => s.showComponentBodies);
  const toggleComponentBodies = useElectronicsStore((s) => s.toggleComponentBodies);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const unplacedComponents = useElectronicsStore((s) => s.unplacedComponents);
  const simulating = useElectronicsStore((s) => s.simulating);

  const setSimulating = useElectronicsStore((s) => s.setSimulating);
  const setSchTool = useElectronicsStore((s) => s.setSchTool);
  const setPcbTool = useElectronicsStore((s) => s.setPcbTool);
  const setSchLabelName = useElectronicsStore((s) => s.setSchLabelName);
  const setPcbActiveLayer = useElectronicsStore((s) => s.setPcbActiveLayer);
  const setPcbGridSize = useElectronicsStore((s) => s.setPcbGridSize);
  const setPcbSnapToGrid = useElectronicsStore((s) => s.setPcbSnapToGrid);
  const setLayerVisible = useElectronicsStore((s) => s.setLayerVisible);

  const syncSchematicToPcb = useDocumentStore((s) => s.syncSchematicToPcb);
  const hasBoard = useDocumentStore((s) => s.document.pcb != null);

  const placeComponent = useCallback((symbolId: string) => {
    useElectronicsStore.setState({
      focusedPane: "schematic",
      schTool: "place",
      schPlacingSymbol: symbolId,
      schPlacingRotation: 0,
    });
  }, []);

  // --- Entry state: not editing a circuit yet --------------------------------
  if (!active) {
    return hasBoard ? (
      <ToolbarButton
        tooltip="Edit the PCB"
        onClick={() => useElectronicsStore.getState().enter()}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
        label="Edit Circuit"
        expanded
      >
        <PencilSimple size={20} />
      </ToolbarButton>
    ) : (
      <ToolbarButton
        tooltip="Start a new circuit"
        onClick={() => useElectronicsStore.getState().startCircuit()}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
        label="New Circuit"
        expanded
      >
        <Cpu size={20} />
      </ToolbarButton>
    );
  }

  // --- Schematic tools -------------------------------------------------------
  if (layout === "schematic") {
    return (
      <>
        <ToolbarButton
          tooltip={simulating ? "Stop simulation" : "Simulate — bring the circuit alive"}
          active={simulating}
          onClick={() => setSimulating(!simulating)}
          iconColor={simulating ? "#ff5a36" : ELECTRONICS_TAB_COLORS.schematic}
        >
          <Lightning size={20} weight={simulating ? "fill" : "regular"} />
        </ToolbarButton>
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
            className="w-20 rounded border border-border bg-transparent px-1 py-0.5 text-[11px] text-text"
            placeholder="Label name"
            aria-label="Label name"
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

        <Divider />

        {/* Place components */}
        {symbols.map((sym) => (
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
  }

  // --- Board tools -----------------------------------------------------------
  const copperLayers = pcbLayers.filter((l) => l.layer.endsWith("Cu"));
  return (
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
      <ToolbarButton
        tooltip="Auto-route ratsnest"
        onClick={() => {
          void autorouteRatsnest();
        }}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <Lightning size={20} />
      </ToolbarButton>
      {unplacedComponents.length > 0 && (
        <ToolbarButton
          tooltip={`Place ${unplacedComponents.length} unplaced component(s)`}
          onClick={() => {
            const boardNodeId = useCoreElectronicsStore.getState().activeBoardNodeId;
            if (boardNodeId != null) {
              syncSchematicToPcb(boardNodeId, useElectronicsStore.getState().netlist ?? undefined);
            }
          }}
          iconColor={ELECTRONICS_TAB_COLORS.pcb}
        >
          <ArrowSquareDown size={20} />
        </ToolbarButton>
      )}

      <Divider />

      {/* Active layer */}
      <div className="flex items-center gap-1 px-1 shrink-0">
        <span className="text-[10px] text-text-muted">Layer</span>
        <select
          className="rounded border border-border bg-transparent px-1 py-0.5 text-[10px] text-text"
          value={pcbActiveLayer}
          onChange={(e) => setPcbActiveLayer(e.target.value as PcbLayer)}
          onClick={(e) => e.stopPropagation()}
          aria-label="Active layer"
        >
          {copperLayers.map((l) => (
            <option key={l.layer} value={l.layer}>
              {l.layer}
            </option>
          ))}
        </select>
      </div>
      {/* Layer visibility */}
      {copperLayers.map((l) => (
        <ToolbarButton
          key={l.layer}
          tooltip={`${l.layer} ${l.visible ? "(visible)" : "(hidden)"}`}
          active={l.visible}
          onClick={() => setLayerVisible(l.layer, !l.visible)}
          iconColor={l.visible ? undefined : "opacity-30"}
        >
          <span
            className="h-3 w-3 rounded-full border border-border"
            style={{ backgroundColor: l.color }}
          />
        </ToolbarButton>
      ))}

      <Divider />

      {/* Grid + snap */}
      <div className="flex items-center gap-1 px-1 shrink-0">
        <span className="text-[10px] text-text-muted">Grid</span>
        <select
          className="rounded border border-border bg-transparent px-1 py-0.5 text-[10px] text-text"
          value={pcbGridSize}
          onChange={(e) => setPcbGridSize(Number(e.target.value))}
          onClick={(e) => e.stopPropagation()}
          aria-label="Grid size"
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
      <ToolbarButton
        tooltip={showComponentBodies ? "Hide 3D component bodies" : "Show 3D component bodies"}
        active={showComponentBodies}
        onClick={toggleComponentBodies}
        iconColor={ELECTRONICS_TAB_COLORS.pcb}
      >
        <Cube size={18} />
      </ToolbarButton>
    </>
  );
}

export default CircuitTabTools;
