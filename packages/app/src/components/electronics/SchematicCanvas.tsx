/**
 * SVG-based schematic canvas.
 *
 * Renders schematic components, wires, junctions, and labels.
 * Implements Principle 2 (net-centric selection) with cross-probe highlights.
 */

import { useRef, useCallback, useState, useMemo } from "react";
import { useDocumentStore } from "@vcad/core";
import type { SchematicComponent, SchematicWire } from "@vcad/ir";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useTheme } from "@/hooks/useTheme";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const COMPONENT_WIDTH = 40;
const COMPONENT_HEIGHT = 30;
const PIN_STUB_LEN = 8;
const SCH_GRID = 10;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Determine which net a component pin belongs to (from netlist). */
function getNetForPin(
  ref: string,
  pinNum: string,
  netlist: { nets: { name: string; connections: { component_ref: string; pin_number: string }[] }[] } | null,
): string | null {
  if (!netlist) return null;
  for (const net of netlist.nets) {
    for (const conn of net.connections) {
      if (conn.component_ref === ref && conn.pin_number === pinNum) {
        return net.name;
      }
    }
  }
  return null;
}

/** Get all nets connected to a component. */
function getNetsForComponent(
  ref: string,
  netlist: { nets: { name: string; connections: { component_ref: string; pin_number: string }[] }[] } | null,
): Set<string> {
  const nets = new Set<string>();
  if (!netlist) return nets;
  for (const net of netlist.nets) {
    for (const conn of net.connections) {
      if (conn.component_ref === ref) {
        nets.add(net.name);
        break;
      }
    }
  }
  return nets;
}

/** Get the net for a wire based on endpoint proximity to pins. */
function getNetForWire(
  wire: SchematicWire,
  netlist: { nets: { name: string; connections: { component_ref: string; pin_number: string }[] }[] } | null,
  components: SchematicComponent[],
): string | null {
  if (!netlist) return null;
  // Check each component pin to see if the wire connects
  for (const comp of components) {
    for (const pin of comp.pins) {
      const px = comp.position.x + pin.position.x;
      const py = comp.position.y + pin.position.y;
      const d1 = Math.hypot(wire.start.x - px, wire.start.y - py);
      const d2 = Math.hypot(wire.end.x - px, wire.end.y - py);
      if (d1 < 1 || d2 < 1) {
        const net = getNetForPin(comp.ref, pin.number, netlist);
        if (net) return net;
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function SchematicCanvas() {
  const svgRef = useRef<SVGSVGElement>(null);
  const { isDark } = useTheme();

  const schematic = useDocumentStore((s) => s.document.schematic);
  const selection = useElectronicsStore((s) => s.selection);
  const hoveredNet = useElectronicsStore((s) => s.hoveredNet);
  const netlist = useElectronicsStore((s) => s.netlist);
  const zoom = useElectronicsStore((s) => s.schZoom);
  const pan = useElectronicsStore((s) => s.schPan);
  const select = useElectronicsStore((s) => s.select);
  const setHoveredNet = useElectronicsStore((s) => s.setHoveredNet);
  const adjustZoom = useElectronicsStore((s) => s.adjustSchZoom);
  const adjustPan = useElectronicsStore((s) => s.adjustSchPan);

  const [dragging, setDragging] = useState(false);
  const dragStart = useRef<{ x: number; y: number } | null>(null);

  // Active net from selection
  const activeNet = useMemo(() => {
    if (selection.type === "net") return selection.netId;
    if (selection.type === "trace" || selection.type === "via" || selection.type === "pad")
      return selection.net;
    return null;
  }, [selection]);

  // Component nets lookup
  const activeComponentRef = useMemo(() => {
    if (selection.type === "component") return selection.ref;
    if (selection.type === "footprint") return selection.ref;
    return null;
  }, [selection]);

  const activeComponentNets = useMemo(() => {
    if (!activeComponentRef) return new Set<string>();
    return getNetsForComponent(activeComponentRef, netlist);
  }, [activeComponentRef, netlist]);

  // Scroll zoom
  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      e.preventDefault();
      adjustZoom(e.deltaY > 0 ? -0.1 : 0.1);
    },
    [adjustZoom],
  );

  // Pan
  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (e.button === 1 || (e.button === 0 && e.altKey)) {
        e.preventDefault();
        setDragging(true);
        dragStart.current = { x: e.clientX, y: e.clientY };
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
      }
    },
    [],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (!dragging || !dragStart.current) return;
      const dx = e.clientX - dragStart.current.x;
      const dy = e.clientY - dragStart.current.y;
      dragStart.current = { x: e.clientX, y: e.clientY };
      adjustPan(dx / zoom, dy / zoom);
    },
    [dragging, zoom, adjustPan],
  );

  const onPointerUp = useCallback(() => {
    setDragging(false);
    dragStart.current = null;
  }, []);

  if (!schematic) {
    return (
      <div className="flex items-center justify-center h-full text-text-muted text-sm">
        No schematic data
      </div>
    );
  }

  const colors = {
    bg: isDark ? "#111" : "#fafafa",
    wire: isDark ? "#aaa" : "#333",
    body: isDark ? "#2a2a2a" : "#f0f0f0",
    bodyStroke: isDark ? "#555" : "#999",
    text: isDark ? "#ddd" : "#222",
    textMuted: isDark ? "#888" : "#999",
    accent: "#3b82f6",
    accentGlow: "rgba(59, 130, 246, 0.3)",
    junction: isDark ? "#4CAF50" : "#2E7D32",
    label: isDark ? "#FFEB3B" : "#F57F17",
  };

  const isNetActive = (net: string | null) =>
    net !== null && (net === activeNet || net === hoveredNet || activeComponentNets.has(net));

  return (
    <svg
      ref={svgRef}
      className="w-full h-full"
      style={{ background: colors.bg, cursor: dragging ? "grabbing" : "default" }}
      onWheel={onWheel}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      <g transform={`translate(${pan.x * zoom + 200}, ${pan.y * zoom + 200}) scale(${zoom})`}>
        {/* Grid */}
        <defs>
          <pattern id="sch-grid" width={SCH_GRID} height={SCH_GRID} patternUnits="userSpaceOnUse">
            <circle cx={SCH_GRID / 2} cy={SCH_GRID / 2} r={0.5} fill={isDark ? "#222" : "#ddd"} />
          </pattern>
        </defs>
        <rect x={-1000} y={-1000} width={2000} height={2000} fill="url(#sch-grid)" />

        {/* Wires */}
        {schematic.wires.map((wire, i) => {
          const net = getNetForWire(wire, netlist, schematic.components);
          const highlight = isNetActive(net);
          return (
            <line
              key={`w-${i}`}
              x1={wire.start.x}
              y1={wire.start.y}
              x2={wire.end.x}
              y2={wire.end.y}
              stroke={highlight ? colors.accent : colors.wire}
              strokeWidth={highlight ? 2.5 : 2}
              style={highlight ? { filter: `drop-shadow(0 0 4px ${colors.accentGlow})` } : undefined}
              className="cursor-pointer"
              onClick={(e) => {
                e.stopPropagation();
                if (net) select({ type: "net", netId: net });
              }}
              onPointerEnter={() => net && setHoveredNet(net)}
              onPointerLeave={() => setHoveredNet(null)}
            />
          );
        })}

        {/* Junctions */}
        {schematic.junctions.map((j, i) => (
          <circle
            key={`j-${i}`}
            cx={j.position.x}
            cy={j.position.y}
            r={3}
            fill={colors.junction}
          />
        ))}

        {/* Labels */}
        {schematic.labels.map((label, i) => (
          <g key={`l-${i}`} transform={`translate(${label.position.x}, ${label.position.y})`}>
            <rect
              x={-2}
              y={-10}
              width={label.name.length * 6 + 4}
              height={14}
              fill={isNetActive(label.name) ? colors.accent : colors.label}
              opacity={0.2}
              rx={2}
            />
            <text
              fontSize={9}
              fill={isNetActive(label.name) ? colors.accent : colors.label}
              fontFamily="monospace"
              dominantBaseline="middle"
              className="cursor-pointer"
              onClick={(e) => {
                e.stopPropagation();
                select({ type: "net", netId: label.name });
              }}
              onPointerEnter={() => setHoveredNet(label.name)}
              onPointerLeave={() => setHoveredNet(null)}
            >
              {label.name}
            </text>
          </g>
        ))}

        {/* Components */}
        {schematic.components.map((comp, i) => {
          const isSelected =
            activeComponentRef === comp.ref;
          const compNets = getNetsForComponent(comp.ref, netlist);
          const hasActiveNet = [...compNets].some((n) => isNetActive(n));
          const highlighted = isSelected || hasActiveNet;

          const h = Math.max(
            COMPONENT_HEIGHT,
            comp.pins.length * 8,
          );

          return (
            <g
              key={`c-${i}`}
              transform={`translate(${comp.position.x}, ${comp.position.y})${comp.rotation ? ` rotate(${comp.rotation})` : ""}`}
              className="cursor-pointer"
              onClick={(e) => {
                e.stopPropagation();
                select({ type: "component", ref: comp.ref });
              }}
              onPointerEnter={() => {
                const nets = getNetsForComponent(comp.ref, netlist);
                const first = nets.values().next().value;
                if (first) setHoveredNet(first);
              }}
              onPointerLeave={() => setHoveredNet(null)}
            >
              {/* Body */}
              <rect
                x={0}
                y={0}
                width={COMPONENT_WIDTH}
                height={h}
                fill={colors.body}
                stroke={highlighted ? colors.accent : colors.bodyStroke}
                strokeWidth={highlighted ? 2 : 1}
                rx={2}
                style={
                  highlighted
                    ? { filter: `drop-shadow(0 0 6px ${colors.accentGlow})` }
                    : undefined
                }
              />

              {/* Pins */}
              {comp.pins.map((pin, pi) => {
                const isLeft = pi < comp.pins.length / 2;
                const pinIdx = isLeft ? pi : pi - Math.ceil(comp.pins.length / 2);
                const py = 10 + pinIdx * 14;
                const px = isLeft ? 0 : COMPONENT_WIDTH;
                const stubDir = isLeft ? -1 : 1;
                const pinNet = getNetForPin(comp.ref, pin.number, netlist);
                const pinHighlight = isNetActive(pinNet);

                return (
                  <g key={`p-${pi}`}>
                    <line
                      x1={px}
                      y1={py}
                      x2={px + PIN_STUB_LEN * stubDir}
                      y2={py}
                      stroke={pinHighlight ? colors.accent : colors.bodyStroke}
                      strokeWidth={pinHighlight ? 2 : 1.5}
                    />
                    <text
                      x={px + (isLeft ? 3 : -3)}
                      y={py - 3}
                      fontSize={6}
                      fill={colors.textMuted}
                      textAnchor={isLeft ? "start" : "end"}
                      fontFamily="monospace"
                    >
                      {pin.number}
                    </text>
                  </g>
                );
              })}

              {/* Reference */}
              <text
                x={COMPONENT_WIDTH / 2}
                y={-4}
                fontSize={9}
                fontWeight="bold"
                fill={colors.text}
                textAnchor="middle"
                fontFamily="monospace"
              >
                {comp.ref}
              </text>

              {/* Value */}
              <text
                x={COMPONENT_WIDTH / 2}
                y={h + 10}
                fontSize={8}
                fill={colors.textMuted}
                textAnchor="middle"
                fontFamily="monospace"
              >
                {comp.value}
              </text>
            </g>
          );
        })}
      </g>
    </svg>
  );
}
