/**
 * SVG-based PCB canvas.
 *
 * Renders board outline, traces, vias, footprints, zones, DRC markers,
 * ratsnest lines, and route preview.
 *
 * Implements:
 * - Principle 2: Net-centric selection + cross-probe highlights
 * - Principle 3: Constraint-first routing with clearance corridor
 * - Principle 4: Ratsnest lines are clickable (starts routing)
 * - Principle 5: Active layer follows intent (pad click)
 */

import { useRef, useCallback, useState, useMemo } from "react";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import type { Footprint, Vec2 } from "@vcad/ir";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useTheme } from "@/hooks/useTheme";
import { PcbFootprintGroup } from "./PcbFootprintGroup";
import type { LayerConfig } from "@/stores/electronics-store";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function getLayerColor(layers: LayerConfig[], layer: string): string {
  const cfg = layers.find((l) => l.layer === layer);
  return cfg?.color ?? "#888";
}

function isLayerVisible(layers: LayerConfig[], layer: string): boolean {
  const cfg = layers.find((l) => l.layer === layer);
  return cfg?.visible ?? false;
}

/** Compute ratsnest lines: unrouted connections between same-net pads. */
function computeRatsnest(
  footprints: Footprint[],
  netlist: { nets: { name: string; connections: { component_ref: string; pin_number: string }[] }[] } | null,
  traces: { net: string; start: Vec2; end: Vec2 }[],
): { net: string; from: Vec2; to: Vec2; fpRef: string; padNum: string }[] {
  if (!netlist) return [];
  const lines: { net: string; from: Vec2; to: Vec2; fpRef: string; padNum: string }[] = [];

  // Build pad position lookup
  const padPositions = new Map<string, Vec2>();
  for (const fp of footprints) {
    for (const pad of fp.pads) {
      const key = `${fp.ref}:${pad.number}`;
      padPositions.set(key, {
        x: fp.position.x + pad.position.x,
        y: fp.position.y + pad.position.y,
      });
    }
  }

  // For each net with >1 connection, create ratsnest between sequential pads
  for (const net of netlist.nets) {
    if (net.connections.length < 2) continue;

    // Check which pairs are already routed (simplified: any trace with matching net)
    const hasTrace = traces.some((t) => t.net === net.name);
    if (hasTrace) continue;

    for (let i = 0; i < net.connections.length - 1; i++) {
      const a = net.connections[i]!;
      const b = net.connections[i + 1]!;
      const posA = padPositions.get(`${a.component_ref}:${a.pin_number}`);
      const posB = padPositions.get(`${b.component_ref}:${b.pin_number}`);
      if (posA && posB) {
        lines.push({
          net: net.name,
          from: posA,
          to: posB,
          fpRef: a.component_ref,
          padNum: a.pin_number,
        });
      }
    }
  }

  return lines;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function PcbCanvas() {
  const svgRef = useRef<SVGSVGElement>(null);
  const { isDark } = useTheme();

  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const pcbDoc = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(pcbDoc, activeBoardNodeId) : null;
  const selection = useElectronicsStore((s) => s.selection);
  const hoveredNet = useElectronicsStore((s) => s.hoveredNet);
  const netlist = useElectronicsStore((s) => s.netlist);
  const zoom = useElectronicsStore((s) => s.pcbZoom);
  const pan = useElectronicsStore((s) => s.pcbPan);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const pcbGridSize = useElectronicsStore((s) => s.pcbGridSize);
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const routeActive = useElectronicsStore((s) => s.routeActive);
  const routePreview = useElectronicsStore((s) => s.routePreview);
  const routeStartPad = useElectronicsStore((s) => s.routeStartPad);

  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const pcbDragging = useElectronicsStore((s) => s.pcbDragging);
  const pcbSnapToGrid = useElectronicsStore((s) => s.pcbSnapToGrid);

  const select = useElectronicsStore((s) => s.select);
  const setHoveredNet = useElectronicsStore((s) => s.setHoveredNet);
  const adjustZoom = useElectronicsStore((s) => s.adjustPcbZoom);
  const adjustPan = useElectronicsStore((s) => s.adjustPcbPan);
  const startRouteFromRatsnest = useElectronicsStore((s) => s.startRouteFromRatsnest);
  const updateRoutePreview = useElectronicsStore((s) => s.updateRoutePreview);
  const startPcbDrag = useElectronicsStore((s) => s.startPcbDrag);
  const cancelPcbDrag = useElectronicsStore((s) => s.cancelPcbDrag);

  const moveFootprint = useDocumentStore((s) => s.moveFootprint);
  const removeTrace = useDocumentStore((s) => s.removeTrace);
  const removeVia = useDocumentStore((s) => s.removeVia);

  const [dragging, setDragging] = useState(false);
  const [fpDragPreview, setFpDragPreview] = useState<{ x: number; y: number } | null>(null);
  const dragStart = useRef<{ x: number; y: number } | null>(null);

  // Active net
  const activeNet = useMemo(() => {
    if (selection.type === "net") return selection.netId;
    if (selection.type === "trace" || selection.type === "via" || selection.type === "pad")
      return selection.net;
    return null;
  }, [selection]);

  const activeFootprintRef = useMemo(() => {
    if (selection.type === "footprint" || selection.type === "component")
      return selection.ref;
    return null;
  }, [selection]);

  const isNetActive = (net: string) =>
    net === activeNet || net === hoveredNet;

  // Ratsnest lines
  const ratsnest = useMemo(() => {
    if (!pcb) return [];
    return computeRatsnest(pcb.footprints, netlist, pcb.traces);
  }, [pcb, netlist]);

  // Pan/zoom handlers
  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      e.preventDefault();
      adjustZoom(e.deltaY > 0 ? -0.1 : 0.1);
    },
    [adjustZoom],
  );

  /** Convert screen coords to PCB coords */
  const screenToPcb = useCallback(
    (e: { clientX: number; clientY: number }) => {
      if (!svgRef.current) return { x: 0, y: 0 };
      const rect = svgRef.current.getBoundingClientRect();
      let x = (e.clientX - rect.left - 200) / zoom - pan.x;
      let y = (e.clientY - rect.top - 200) / zoom - pan.y;
      if (pcbSnapToGrid) {
        x = Math.round(x / pcbGridSize) * pcbGridSize;
        y = Math.round(y / pcbGridSize) * pcbGridSize;
      }
      return { x, y };
    },
    [zoom, pan, pcbSnapToGrid, pcbGridSize],
  );

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (e.button === 1 || (e.button === 0 && e.altKey)) {
        e.preventDefault();
        setDragging(true);
        dragStart.current = { x: e.clientX, y: e.clientY };
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        return;
      }

      // Move tool: start footprint drag
      if (e.button === 0 && pcbTool === "move" && pcb) {
        const pos = screenToPcb(e);
        // Hit-test footprints (reverse order for top-most)
        for (let i = pcb.footprints.length - 1; i >= 0; i--) {
          const fp = pcb.footprints[i]!;
          // Simple bounding box hit test
          const halfW = 5, halfH = 5; // approximate footprint size
          if (
            pos.x >= fp.position.x - halfW && pos.x <= fp.position.x + halfW &&
            pos.y >= fp.position.y - halfH && pos.y <= fp.position.y + halfH
          ) {
            startPcbDrag(i, fp.position);
            setFpDragPreview(fp.position);
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            return;
          }
        }
      }
    },
    [pcbTool, pcb, screenToPcb, startPcbDrag],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (dragging && dragStart.current) {
        const dx = e.clientX - dragStart.current.x;
        const dy = e.clientY - dragStart.current.y;
        dragStart.current = { x: e.clientX, y: e.clientY };
        adjustPan(dx / zoom, dy / zoom);
        return;
      }

      // Footprint drag preview
      if (pcbDragging) {
        const pos = screenToPcb(e);
        setFpDragPreview(pos);
        return;
      }

      // Route preview
      if (routeActive && svgRef.current) {
        const pos = screenToPcb(e);
        updateRoutePreview([pos]);
      }
    },
    [dragging, zoom, adjustPan, routeActive, updateRoutePreview, pcbDragging, screenToPcb],
  );

  const onPointerUp = useCallback(() => {
    // Finish pan
    if (dragging) {
      setDragging(false);
      dragStart.current = null;
      return;
    }

    // Finish footprint drag
    if (pcbDragging && fpDragPreview && activeBoardNodeId != null) {
      moveFootprint(activeBoardNodeId, pcbDragging.fpIdx, { x: fpDragPreview.x, y: fpDragPreview.y, z: 0 });
      cancelPcbDrag();
      setFpDragPreview(null);
    }
  }, [dragging, pcbDragging, fpDragPreview, moveFootprint, cancelPcbDrag, activeBoardNodeId]);

  const initSchematic = useDocumentStore((s) => s.initSchematic);
  const initPcb = useDocumentStore((s) => s.initPcb);

  if (!pcb) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3">
        <span className="text-text-muted text-sm">No PCB data</span>
        <button
          className="px-3 py-1.5 text-xs bg-accent text-white rounded hover:bg-accent/90 transition-colors"
          onClick={() => {
            initPcb();
            const doc = useDocumentStore.getState().document;
            if (!doc.schematic) initSchematic();
          }}
        >
          New Circuit
        </button>
      </div>
    );
  }

  const accentColor = "#3b82f6";
  const edgeCutsColor = getLayerColor(pcbLayers, "EdgeCuts");

  return (
    <svg
      ref={svgRef}
      className="w-full h-full"
      style={{ background: isDark ? "#0a0a0a" : "#f5f5f5", cursor: dragging ? "grabbing" : "crosshair" }}
      onWheel={onWheel}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onClick={() => select({ type: "none" })}
    >
      <g transform={`translate(${pan.x * zoom + 200}, ${pan.y * zoom + 200}) scale(${zoom})`}>
        {/* Grid */}
        <defs>
          <pattern
            id="pcb-grid"
            width={pcbGridSize}
            height={pcbGridSize}
            patternUnits="userSpaceOnUse"
          >
            <circle
              cx={pcbGridSize / 2}
              cy={pcbGridSize / 2}
              r={pcbGridSize * 0.05}
              fill={isDark ? "#1a1a1a" : "#e0e0e0"}
            />
          </pattern>
        </defs>
        <rect x={-500} y={-500} width={1000} height={1000} fill="url(#pcb-grid)" />

        {/* Board outline */}
        {pcb.outline.vertices.length > 0 && (
          <polygon
            points={pcb.outline.vertices.map((v) => `${v.x},${v.y}`).join(" ")}
            fill="none"
            stroke={edgeCutsColor}
            strokeWidth={0.2}
          />
        )}
        {/* Cutouts */}
        {(pcb.outline.cutouts ?? []).map((cutout, i) => (
          <polygon
            key={`cutout-${i}`}
            points={cutout.map((v) => `${v.x},${v.y}`).join(" ")}
            fill={isDark ? "#0a0a0a" : "#f5f5f5"}
            stroke={edgeCutsColor}
            strokeWidth={0.15}
          />
        ))}

        {/* Zones */}
        {pcb.zones.map((zone, i) => {
          if (!isLayerVisible(pcbLayers, zone.layer)) return null;
          const color = getLayerColor(pcbLayers, zone.layer);
          return (
            <polygon
              key={`zone-${i}`}
              points={zone.outline.map((v) => `${v.x},${v.y}`).join(" ")}
              fill={color}
              opacity={0.15}
              stroke={color}
              strokeWidth={0.1}
            />
          );
        })}

        {/* Traces */}
        {pcb.traces.map((trace, i) => {
          if (!isLayerVisible(pcbLayers, trace.layer)) return null;
          const color = isNetActive(trace.net)
            ? accentColor
            : getLayerColor(pcbLayers, trace.layer);
          return (
            <line
              key={`tr-${i}`}
              x1={trace.start.x}
              y1={trace.start.y}
              x2={trace.end.x}
              y2={trace.end.y}
              stroke={color}
              strokeWidth={trace.width}
              strokeLinecap="round"
              className="cursor-pointer"
              style={
                isNetActive(trace.net)
                  ? { filter: `drop-shadow(0 0 2px ${accentColor}66)` }
                  : undefined
              }
              onClick={(e) => {
                e.stopPropagation();
                if (pcbTool === "delete" && activeBoardNodeId != null) {
                  removeTrace(activeBoardNodeId, i);
                } else {
                  select({ type: "trace", idx: i, net: trace.net });
                }
              }}
              onPointerEnter={() => setHoveredNet(trace.net)}
              onPointerLeave={() => setHoveredNet(null)}
            />
          );
        })}

        {/* Vias */}
        {pcb.vias.map((via, i) => (
          <g
            key={`via-${i}`}
            className="cursor-pointer"
            onClick={(e) => {
              e.stopPropagation();
              if (pcbTool === "delete" && activeBoardNodeId != null) {
                removeVia(activeBoardNodeId, i);
              } else {
                select({ type: "via", idx: i, net: via.net });
              }
            }}
            onPointerEnter={() => setHoveredNet(via.net)}
            onPointerLeave={() => setHoveredNet(null)}
          >
            <circle
              cx={via.position.x}
              cy={via.position.y}
              r={via.diameter / 2}
              fill={isNetActive(via.net) ? accentColor : "#888"}
              opacity={0.9}
              style={
                isNetActive(via.net)
                  ? { filter: `drop-shadow(0 0 2px ${accentColor}66)` }
                  : undefined
              }
            />
            <circle
              cx={via.position.x}
              cy={via.position.y}
              r={via.drill / 2}
              fill="#111"
            />
          </g>
        ))}

        {/* Footprints */}
        {pcb.footprints.map((fp, i) => {
          // Show dragged footprint at preview position
          const displayFp = (pcbDragging?.fpIdx === i && fpDragPreview)
            ? { ...fp, position: fpDragPreview }
            : fp;
          return (
            <g key={`fp-${i}`} opacity={pcbDragging?.fpIdx === i ? 0.7 : 1}>
              <PcbFootprintGroup
                footprint={displayFp}
                layers={pcbLayers}
                highlight={activeFootprintRef === fp.ref}
                accentColor={accentColor}
              />
            </g>
          );
        })}

        {/* DRC markers */}
        {drcViolations.map((v, i) => (
          <g key={`drc-${i}`}>
            <circle
              cx={v.position.x}
              cy={v.position.y}
              r={0.8}
              fill="none"
              stroke={v.severity === "Error" ? "#ef4444" : "#f59e0b"}
              strokeWidth={0.2}
              className="cursor-pointer"
              onClick={(e) => {
                e.stopPropagation();
                // Could select DRC marker; for now just stop propagation
              }}
            >
              <animate
                attributeName="r"
                values="0.6;1.0;0.6"
                dur="1.5s"
                repeatCount="indefinite"
              />
            </circle>
            <title>{v.message}</title>
          </g>
        ))}

        {/* Ratsnest (Principle 4: clickable affordance) */}
        {ratsnest.map((r, i) => (
          <line
            key={`rat-${i}`}
            x1={r.from.x}
            y1={r.from.y}
            x2={r.to.x}
            y2={r.to.y}
            stroke={isNetActive(r.net) ? accentColor : isDark ? "#555" : "#bbb"}
            strokeWidth={0.15}
            strokeDasharray="0.5 0.5"
            className="cursor-pointer"
            onClick={(e) => {
              e.stopPropagation();
              startRouteFromRatsnest(r.fpRef, r.padNum, r.net);
            }}
            onPointerEnter={() => setHoveredNet(r.net)}
            onPointerLeave={() => setHoveredNet(null)}
          >
            <title>Click to route {r.net}</title>
          </line>
        ))}

        {/* Route preview (Principle 3: clearance corridor) */}
        {routeActive && routeStartPad && routePreview.length > 0 && (() => {
          // Find start pad position
          const fp = pcb.footprints.find((f) => f.ref === routeStartPad.fpRef);
          const pad = fp?.pads.find((p) => p.number === routeStartPad.padNum);
          if (!fp || !pad) return null;

          const startPos = {
            x: fp.position.x + pad.position.x,
            y: fp.position.y + pad.position.y,
          };
          const endPos = routePreview[routePreview.length - 1]!;
          const traceWidth = pcb.rules.defaultRules.traceWidth;
          const clearance = pcb.rules.defaultRules.clearance;

          return (
            <g pointerEvents="none">
              {/* Clearance corridor */}
              <line
                x1={startPos.x}
                y1={startPos.y}
                x2={endPos.x}
                y2={endPos.y}
                stroke={accentColor}
                strokeWidth={traceWidth + clearance * 2}
                strokeLinecap="round"
                opacity={0.15}
              />
              {/* Trace preview */}
              <line
                x1={startPos.x}
                y1={startPos.y}
                x2={endPos.x}
                y2={endPos.y}
                stroke={accentColor}
                strokeWidth={traceWidth}
                strokeLinecap="round"
                opacity={0.7}
                strokeDasharray="1 0.5"
              />
            </g>
          );
        })()}
      </g>
    </svg>
  );
}
