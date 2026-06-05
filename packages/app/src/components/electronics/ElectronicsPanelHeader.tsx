/**
 * Persistent header for the electronics property panel.
 *
 * Consolidates what used to be two floating HUDs — the top-left status overlay
 * (mode/tool, DRC/ERC, active layer) and the top-right "Board Overview" card
 * (footprint/trace/via/net/layer/component counts) — into a single strip that
 * sits above the contextual panel body (a selected ECAD item, or the board
 * transform when nothing is selected). Shown whenever electronics is active.
 */

import { useState } from "react";
import {
  useDocumentStore,
  useCoreElectronicsStore,
  useEngineStore,
  getNodePcb,
  isPcbBoardPart,
  findPcbBoardPart,
  getPcbBoardTransform,
} from "@vcad/core";
import type { Pcb } from "@vcad/ir";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useNotificationStore } from "@/stores/notification-store";
import { aabbOfPositions, mergeAabbs, type Aabb } from "@/lib/pcb-interference";
import { computeBoardFit } from "@/lib/pcb-fit";

/** Clearance (mm) inset between the board edge and the enclosure walls on a fit. */
const ENCLOSURE_CLEARANCE = 2;

/** W×H (mm) of a board outline's bounding box. */
function outlineWH(pcb: Pcb): { w: number; h: number } {
  const verts = pcb.outline.vertices;
  if (verts.length < 3) return { w: 0, h: 0 };
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  for (const v of verts) {
    if (v.x < minX) minX = v.x;
    if (v.x > maxX) maxX = v.x;
    if (v.y < minY) minY = v.y;
    if (v.y > maxY) maxY = v.y;
  }
  return { w: maxX - minX, h: maxY - minY };
}

function Stat({
  label,
  value,
  danger,
}: {
  label: string;
  value: number;
  danger?: boolean;
}) {
  return (
    <span className="inline-flex items-baseline gap-1">
      <span className="text-text-muted">{label}</span>
      <span className={danger && value > 0 ? "text-danger font-medium" : "text-text font-medium"}>
        {value}
      </span>
    </span>
  );
}

/**
 * Editable board-size chip: shows W×H, click to edit, plus a "Fit" button that
 * sizes + positions the board to the surrounding mechanical parts (the ECAD↔MCAD
 * co-design link). Resizing re-extrudes the FR4 slab via the kernel.
 */
function BoardSizeControl({ pcb }: { pcb: Pcb }) {
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const parts = useDocumentStore((s) => s.parts);
  const scene = useEngineStore((s) => s.scene);
  const resizeBoard = useDocumentStore((s) => s.resizeBoard);
  const setTranslation = useDocumentStore((s) => s.setTranslation);
  const addToast = useNotificationStore((s) => s.addToast);

  const [editing, setEditing] = useState<{ w: string; h: string } | null>(null);

  const { w, h } = outlineWH(pcb);

  // Count non-board mechanical parts that actually carry a mesh, so "Fit" only
  // shows once there's geometry to fit to (gating on part identity alone races
  // the mesh eval — the button would appear a frame before fitting can work).
  let mechCount = 0;
  scene?.parts.forEach((ep, idx) => {
    const pi = parts[idx];
    if (pi && !isPcbBoardPart(pi) && (ep.mesh?.positions?.length ?? 0) > 0) mechCount++;
  });

  const commit = () => {
    if (!editing) return;
    const nw = parseFloat(editing.w);
    const nh = parseFloat(editing.h);
    if (isFinite(nw) && isFinite(nh) && nw > 0 && nh > 0) {
      resizeBoard(nw, nh);
    }
    setEditing(null);
  };

  const fitToEnclosure = () => {
    // Gather world-space AABBs of every non-board part, union into the
    // enclosure box, then size + place the board to fill its XY footprint.
    const mech: Aabb[] = [];
    scene?.parts.forEach((ep, idx) => {
      const pi = parts[idx];
      if (pi && isPcbBoardPart(pi)) return;
      const bb = aabbOfPositions(ep.mesh?.positions);
      if (bb) mech.push(bb);
    });
    const enc = mergeAabbs(mech);
    if (!enc) {
      addToast("No surrounding parts to fit the board to", "info");
      return;
    }

    // Size + place through the board's full transform so a board that's been
    // moved, scaled, or rotated in the assembly still fits correctly.
    const boardPart =
      activeBoardNodeId != null ? findPcbBoardPart(parts, activeBoardNodeId) : null;
    const xf = boardPart
      ? getPcbBoardTransform(document, boardPart)
      : { position: { x: 0, y: 0, z: 0 }, rotationDeg: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } };
    const fit = computeBoardFit(enc, xf, ENCLOSURE_CLEARANCE);
    if (!fit) {
      addToast("Surrounding parts are too small to fit a board", "info");
      return;
    }

    resizeBoard(fit.width, fit.height);
    if (boardPart) setTranslation(boardPart.id, fit.position);
    addToast(
      `Board fit to enclosure · ${Math.round(fit.width)}×${Math.round(fit.height)}mm`,
      "success",
    );
  };

  if (editing) {
    return (
      <span className="inline-flex items-center gap-0.5">
        <input
          aria-label="Board width (mm)"
          type="number"
          autoFocus
          value={editing.w}
          onChange={(e) => setEditing({ ...editing, w: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") setEditing(null);
          }}
          className="w-10 bg-surface border border-border rounded px-1 py-0.5 text-text text-[11px] tabular-nums"
        />
        <span className="text-text-muted">×</span>
        <input
          aria-label="Board height (mm)"
          type="number"
          value={editing.h}
          onChange={(e) => setEditing({ ...editing, h: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") setEditing(null);
          }}
          className="w-10 bg-surface border border-border rounded px-1 py-0.5 text-text text-[11px] tabular-nums"
        />
        <button
          onClick={commit}
          className="ml-0.5 px-1 py-0.5 rounded text-accent hover:bg-accent/10"
          title="Apply board size"
        >
          ✓
        </button>
      </span>
    );
  }

  return (
    <span className="inline-flex items-center gap-1">
      <button
        onClick={() => setEditing({ w: String(Math.round(w)), h: String(Math.round(h)) })}
        className="text-text-muted hover:text-text underline decoration-dotted underline-offset-2"
        title="Edit board size"
      >
        {Math.round(w)}×{Math.round(h)}mm
      </button>
      {mechCount > 0 && (
        <button
          onClick={fitToEnclosure}
          className="px-1 py-0.5 rounded border border-border text-text-muted hover:text-text hover:border-accent/60"
          title="Resize + position the board to fit the surrounding parts"
        >
          Fit
        </button>
      )}
    </span>
  );
}

export function ElectronicsPanelHeader() {
  const focusedPane = useElectronicsStore((s) => s.focusedPane);
  const schTool = useElectronicsStore((s) => s.schTool);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const ercViolations = useElectronicsStore((s) => s.ercViolations);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;
  const schematic = document.schematic;

  const currentTool = focusedPane === "pcb" ? pcbTool : schTool;
  const toolLabel = currentTool.charAt(0).toUpperCase() + currentTool.slice(1);

  const drcErrors = drcViolations.filter((v) => v.severity === "Error").length;
  const drcWarnings = drcViolations.length - drcErrors;
  const ercCount = ercViolations.length;
  const activeLayerConfig = pcbLayers.find((l) => l.layer === pcbActiveLayer);

  return (
    <div className="shrink-0 border-b border-border/40 bg-surface/60 px-3 py-2 text-[11px]">
      {/* Title + health */}
      <div className="flex items-center justify-between gap-2">
        <span className="flex items-center gap-1.5 min-w-0">
          <span className="font-medium text-text">Circuit</span>
          {pcb && <BoardSizeControl pcb={pcb} />}
        </span>
        <div className="flex items-center gap-2 shrink-0">
          <span className="inline-flex items-center gap-1" title="DRC errors / warnings">
            <span className="text-text-muted">DRC</span>
            <span className={drcErrors > 0 ? "text-danger font-medium" : "text-text-muted"}>
              {drcErrors}
            </span>
            <span className={drcWarnings > 0 ? "text-warning" : "text-text-muted"}>
              ⚠{drcWarnings}
            </span>
          </span>
          <span className="inline-flex items-center gap-1" title="ERC issues">
            <span className="text-text-muted">ERC</span>
            <span className={ercCount > 0 ? "text-warning font-medium" : "text-text-muted"}>
              {ercCount}
            </span>
          </span>
        </div>
      </div>

      {/* Mode · tool · active layer */}
      <div className="mt-0.5 flex items-center gap-1.5 text-text-muted">
        <span className="capitalize">{focusedPane}</span>
        <span>·</span>
        <span className="text-text">{toolLabel}</span>
        {focusedPane === "pcb" && activeLayerConfig && (
          <span className="ml-auto inline-flex items-center gap-1" title="Active layer">
            <span
              className="w-2 h-2 rounded-full"
              style={{ backgroundColor: activeLayerConfig.color }}
            />
            <span>{pcbActiveLayer}</span>
          </span>
        )}
      </div>

      {/* Board overview counts */}
      <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5">
        {pcb && (
          <>
            <Stat label="Footprints" value={pcb.footprints.length} />
            <Stat label="Traces" value={pcb.traces.length} />
            <Stat label="Vias" value={pcb.vias.length} />
            <Stat label="Nets" value={pcb.nets.length} />
            <Stat
              label="Layers"
              value={pcb.stackup.layers.filter((l) => l.copperThickness).length}
            />
          </>
        )}
        {schematic && <Stat label="Components" value={schematic.components.length} />}
      </div>
    </div>
  );
}

export default ElectronicsPanelHeader;
