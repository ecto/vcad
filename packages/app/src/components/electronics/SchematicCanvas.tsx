/**
 * SVG-based schematic canvas.
 *
 * Renders schematic components, wires, junctions, and labels.
 * Implements Principle 2 (net-centric selection) with cross-probe highlights.
 * Supports place, wire, label, delete, and move tools.
 */

import { useRef, useCallback, useState, useMemo, useEffect } from "react";
import { useDocumentStore, getPcbNodeIds } from "@vcad/core";
import type { SchematicComponent, SchematicWire } from "@vcad/ir";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useTheme } from "@/hooks/useTheme";
import { getSymbol } from "./symbol-library";
import type { SymbolGraphic } from "./symbol-library";

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

/** Convert screen coordinates to SVG schematic coordinates. */
function screenToSvg(
  e: { clientX: number; clientY: number },
  svgEl: SVGSVGElement,
  zoom: number,
  pan: { x: number; y: number },
): { x: number; y: number } {
  const rect = svgEl.getBoundingClientRect();
  const sx = (e.clientX - rect.left - 200) / zoom - pan.x;
  const sy = (e.clientY - rect.top - 200) / zoom - pan.y;
  return { x: sx, y: sy };
}

/** Snap a value to the nearest grid point. */
function snapToGrid(v: number, grid: number): number {
  return Math.round(v / grid) * grid;
}

/** Snap to nearest component pin if within threshold, otherwise grid-snap. */
function snapToGridOrPin(
  pos: { x: number; y: number },
  components: SchematicComponent[],
  grid: number,
  threshold: number = 12,
): { x: number; y: number; isPin: boolean } {
  let bestDist = threshold;
  let bestPos = { x: snapToGrid(pos.x, grid), y: snapToGrid(pos.y, grid), isPin: false };
  for (const comp of components) {
    for (const pin of comp.pins) {
      const px = comp.position.x + pin.position.x;
      const py = comp.position.y + pin.position.y;
      const d = Math.hypot(pos.x - px, pos.y - py);
      if (d < bestDist) {
        bestDist = d;
        bestPos = { x: px, y: py, isPin: true };
      }
    }
  }
  return bestPos;
}

/** Check if point p lies on segment (a,b) — excluding endpoints — within tolerance. */
function pointOnSegment(
  p: { x: number; y: number },
  a: { x: number; y: number },
  b: { x: number; y: number },
  tol: number = 2,
): boolean {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const lenSq = dx * dx + dy * dy;
  if (lenSq < 0.01) return false;
  const t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / lenSq;
  if (t < 0.01 || t > 0.99) return false;
  const proj = { x: a.x + t * dx, y: a.y + t * dy };
  return Math.hypot(p.x - proj.x, p.y - proj.y) < tol;
}

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
  for (const comp of components) {
    for (const pin of comp.pins) {
      const px = comp.position.x + pin.position.x;
      const py = comp.position.y + pin.position.y;
      const d1 = Math.hypot(wire.start.x - px, wire.start.y - py);
      const d2 = Math.hypot(wire.end.x - px, wire.end.y - py);
      if (d1 < 2 || d2 < 2) {
        const net = getNetForPin(comp.ref, pin.number, netlist);
        if (net) return net;
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Symbol graphics renderer
// ---------------------------------------------------------------------------

/** Map a node voltage to a colour: blue (low) → green → red (high rail). */
/** Compact engineering formatter for annotation labels (e.g. "3.30V", "1.2mA"). */
function fmtEng(v: number, unit: string): string {
  const a = Math.abs(v);
  if (a >= 1 || a === 0) return `${v.toPrecision(3)}${unit}`;
  if (a >= 1e-3) return `${(v * 1e3).toPrecision(3)}m${unit}`;
  if (a >= 1e-6) return `${(v * 1e6).toPrecision(3)}µ${unit}`;
  if (a >= 1e-9) return `${(v * 1e9).toPrecision(3)}n${unit}`;
  return `${v.toExponential(1)}${unit}`;
}

function voltageColor(v: number, vmax = 5): string {
  const t = Math.max(0, Math.min(1, Math.abs(v) / vmax));
  return `hsl(${240 * (1 - t)}, 90%, 58%)`;
}

function renderSymbolGraphics(
  graphics: SymbolGraphic[],
  stroke: string,
  fill: string,
) {
  return graphics.map((g, i) => {
    switch (g.type) {
      case "rect":
        return (
          <rect
            key={i}
            x={g.x}
            y={g.y}
            width={g.width}
            height={g.height}
            fill={fill}
            stroke={stroke}
            strokeWidth={1}
            rx={1}
          />
        );
      case "line":
        return (
          <line
            key={i}
            x1={g.x1}
            y1={g.y1}
            x2={g.x2}
            y2={g.y2}
            stroke={stroke}
            strokeWidth={1.5}
          />
        );
      case "circle":
        return (
          <circle
            key={i}
            cx={g.cx}
            cy={g.cy}
            r={g.r}
            fill="none"
            stroke={stroke}
            strokeWidth={1}
          />
        );
      case "polyline":
        return (
          <polyline
            key={i}
            points={(g.points ?? []).map((p) => `${p.x},${p.y}`).join(" ")}
            fill="none"
            stroke={stroke}
            strokeWidth={1.5}
          />
        );
      default:
        return null;
    }
  });
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
  // Live circuit simulation state ("come alive")
  const simulating = useElectronicsStore((s) => s.simulating);
  const simNodeVoltages = useElectronicsStore((s) => s.simNodeVoltages);
  const simDeviceCurrents = useElectronicsStore((s) => s.simDeviceCurrents);
  const simRotorAngles = useElectronicsStore((s) => s.simRotorAngles);
  const simNetToNode = useElectronicsStore((s) => s.simNetToNode);
  const simRefToDevice = useElectronicsStore((s) => s.simRefToDevice);
  const simOn = simulating && simNodeVoltages != null;
  // One-shot DC/AC analysis (Analyze flow): annotations + fail-closed blockers
  const analysis = useElectronicsStore((s) => s.analysis);
  const dcOn =
    !simOn && analysis.status === "ok" && analysis.showDcAnnotations && analysis.dc != null;
  const blockedRefs = useMemo(
    () => new Set(analysis.blockers.map((b) => b.reference)),
    [analysis.blockers],
  );
  const blockerMessage = (ref: string) =>
    analysis.blockers.find((b) => b.reference === ref)?.message ?? null;
  /** DC current through a component from the last analysis, or null. */
  const dcCompCurrent = (ref: string): number | null => {
    if (!dcOn || !analysis.mapping || !analysis.dc) return null;
    const id = analysis.mapping.deviceOfRef[ref];
    return id == null ? null : (analysis.dc.deviceCurrents[id] ?? null);
  };
  /** Open the tune dialog on right-click when the component is simulated. */
  const onComponentContextMenu = (e: React.MouseEvent, ref: string) => {
    if (analysis.status !== "ok" || analysis.mapping?.deviceOfRef[ref] == null) return;
    e.preventDefault();
    e.stopPropagation();
    useElectronicsStore.getState().setAnalysis({ tuningRef: ref, tuneResult: null });
  };
  /** Voltage of a net under simulation (null when not simulating / unknown). */
  const netVoltage = (net: string | null): number | null => {
    if (!simOn || !net || !simNetToNode || !simNodeVoltages) return null;
    const node = simNetToNode[net];
    return node == null ? null : (simNodeVoltages[node] ?? null);
  };
  /** Current through a component under simulation (A), or null. */
  const compCurrent = (ref: string): number | null => {
    if (!simOn || !simRefToDevice || !simDeviceCurrents) return null;
    const id = simRefToDevice[ref];
    return id == null ? null : (simDeviceCurrents[id] ?? null);
  };
  /** Rotor angle (rad) of a motor component under simulation, else 0. */
  const motorAngle = (ref: string): number => {
    if (!simOn || !simRefToDevice || !simRotorAngles) return 0;
    const id = simRefToDevice[ref];
    return id == null ? 0 : (simRotorAngles[id] ?? 0);
  };
  const zoom = useElectronicsStore((s) => s.schZoom);
  const pan = useElectronicsStore((s) => s.schPan);
  const schTool = useElectronicsStore((s) => s.schTool);
  const placingSymbol = useElectronicsStore((s) => s.schPlacingSymbol);
  const placingRotation = useElectronicsStore((s) => s.schPlacingRotation);
  const wireStart = useElectronicsStore((s) => s.schWireStart);
  const wirePreview = useElectronicsStore((s) => s.schWirePreview);

  const select = useElectronicsStore((s) => s.select);
  const setHoveredNet = useElectronicsStore((s) => s.setHoveredNet);

  // Escape cancels an in-progress wire (the universal EDA reflex; complements
  // the right-click cancel and double-click finish).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && schTool === "wire") {
        useElectronicsStore.getState().cancelSchWire();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [schTool]);
  const zoomAt = useElectronicsStore((s) => s.zoomSchAt);
  const adjustPan = useElectronicsStore((s) => s.adjustSchPan);
  const startSchWire = useElectronicsStore((s) => s.startSchWire);
  const updateSchWirePreview = useElectronicsStore((s) => s.updateSchWirePreview);
  const nextRef = useElectronicsStore((s) => s.nextRef);

  const addSchematicComponent = useDocumentStore((s) => s.addSchematicComponent);
  const removeSchematicComponent = useDocumentStore((s) => s.removeSchematicComponent);
  const addSchematicWire = useDocumentStore((s) => s.addSchematicWire);
  const removeSchematicWire = useDocumentStore((s) => s.removeSchematicWire);
  const addSchematicLabel = useDocumentStore((s) => s.addSchematicLabel);
  const removeSchematicLabel = useDocumentStore((s) => s.removeSchematicLabel);
  const moveSchematicComponent = useDocumentStore((s) => s.moveSchematicComponent);
  const moveSchematicComponentWithWires = useDocumentStore((s) => s.moveSchematicComponentWithWires);
  const addSchematicJunction = useDocumentStore((s) => s.addSchematicJunction);
  const initSchematic = useDocumentStore((s) => s.initSchematic);
  const initPcb = useDocumentStore((s) => s.initPcb);

  const [dragging, setDragging] = useState(false);
  const [ghostPos, setGhostPos] = useState<{ x: number; y: number } | null>(null);
  const [moveDrag, setMoveDrag] = useState<{ compIdx: number; offset: { x: number; y: number } } | null>(null);
  const [moveConnections, setMoveConnections] = useState<{ wireIdx: number; endpoint: "start" | "end"; pinOffset: { x: number; y: number } }[]>([]);
  const [snapTarget, setSnapTarget] = useState<{ x: number; y: number; isPin: boolean } | null>(null);
  const dragStart = useRef<{ x: number; y: number } | null>(null);

  // Active net from selection
  const activeNet = useMemo(() => {
    if (selection.type === "net") return selection.netId;
    if (selection.type === "trace" || selection.type === "via" || selection.type === "pad")
      return selection.net;
    return null;
  }, [selection]);

  const activeComponentRef = useMemo(() => {
    if (selection.type === "component") return selection.ref;
    if (selection.type === "footprint") return selection.ref;
    return null;
  }, [selection]);

  const activeComponentNets = useMemo(() => {
    if (!activeComponentRef) return new Set<string>();
    return getNetsForComponent(activeComponentRef, netlist);
  }, [activeComponentRef, netlist]);

  const isNetActive = (net: string | null) =>
    net !== null && (net === activeNet || net === hoveredNet || activeComponentNets.has(net));

  // DC annotation anchors: one voltage label per net, at the midpoint of the
  // first wire carrying that net.
  const dcNetAnchors = useMemo(() => {
    if (!dcOn || !schematic || !analysis.mapping || !analysis.dc) return [];
    const seen = new Set<string>();
    const out: Array<{ net: string; x: number; y: number; v: number }> = [];
    for (const wire of schematic.wires) {
      const net = getNetForWire(wire, netlist, schematic.components);
      if (!net || seen.has(net)) continue;
      const node = analysis.mapping.nodeOfNet[net];
      if (node == null || node === 0) continue;
      const v = analysis.dc.nodeVoltages[node];
      if (v == null) continue;
      seen.add(net);
      out.push({
        net,
        x: (wire.start.x + wire.end.x) / 2,
        y: (wire.start.y + wire.end.y) / 2,
        v,
      });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dcOn, schematic, netlist, analysis.mapping, analysis.dc]);

  // Connection dots: where wire endpoints meet component pins
  const connectionDots = useMemo(() => {
    if (!schematic) return [];
    const dots = new Map<string, { x: number; y: number }>();
    for (const wire of schematic.wires) {
      for (const comp of schematic.components) {
        for (const pin of comp.pins) {
          const px = comp.position.x + pin.position.x;
          const py = comp.position.y + pin.position.y;
          for (const ep of [wire.start, wire.end]) {
            if (Math.hypot(ep.x - px, ep.y - py) < 2) {
              dots.set(`${px},${py}`, { x: px, y: py });
            }
          }
        }
      }
    }
    return [...dots.values()];
  }, [schematic]);

  // Wire overrides during component move drag (rubber-banding)
  const wireOverrides = useMemo(() => {
    const map = new Map<number, { start?: { x: number; y: number }; end?: { x: number; y: number } }>();
    if (!moveDrag || !ghostPos || moveConnections.length === 0) return map;
    for (const conn of moveConnections) {
      const newPos = { x: ghostPos.x + conn.pinOffset.x, y: ghostPos.y + conn.pinOffset.y };
      const existing = map.get(conn.wireIdx) ?? {};
      existing[conn.endpoint] = newPos;
      map.set(conn.wireIdx, existing);
    }
    return map;
  }, [moveDrag, ghostPos, moveConnections]);

  // Scroll zoom / pan
  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      e.preventDefault();
      if (!svgRef.current) return;
      const rect = svgRef.current.getBoundingClientRect();
      const cx = e.clientX - rect.left - 200;
      const cy = e.clientY - rect.top - 200;

      if (e.ctrlKey || e.metaKey) {
        // Pinch-to-zoom (trackpad) or Ctrl+scroll — zoom toward cursor
        const delta = -e.deltaY * 0.01;
        zoomAt(delta, cx, cy);
      } else if (e.deltaMode === 1) {
        // Mouse wheel (line-based) — zoom toward cursor
        zoomAt(e.deltaY > 0 ? -0.1 : 0.1, cx, cy);
      } else {
        // Trackpad two-finger scroll — pan
        adjustPan(-e.deltaX / zoom, -e.deltaY / zoom);
      }
    },
    [zoom, zoomAt, adjustPan],
  );

  // Pointer events
  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Middle-click or alt+click for panning
      if (e.button === 1 || (e.button === 0 && e.altKey)) {
        e.preventDefault();
        setDragging(true);
        dragStart.current = { x: e.clientX, y: e.clientY };
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        return;
      }

      if (e.button !== 0 || !svgRef.current) return;
      const pos = screenToSvg(e, svgRef.current, zoom, pan);

      // Move tool: start drag on component
      if (schTool === "move" && !moveDrag) {
        const comps = schematic?.components ?? [];
        for (let ci = comps.length - 1; ci >= 0; ci--) {
          const c = comps[ci]!;
          const sym = c.properties?.symbolId ? getSymbol(c.properties.symbolId) : null;
          const w = sym ? 56 : COMPONENT_WIDTH;
          const h = sym ? Math.max(30, c.pins.length * 14 + 10) : Math.max(COMPONENT_HEIGHT, c.pins.length * 8);
          if (pos.x >= c.position.x - 10 && pos.x <= c.position.x + w + 10 &&
              pos.y >= c.position.y - 10 && pos.y <= c.position.y + h + 10) {
            // Find connected wire endpoints for rubber-banding
            const wires = schematic?.wires ?? [];
            const conns: { wireIdx: number; endpoint: "start" | "end"; pinOffset: { x: number; y: number } }[] = [];
            for (const pin of c.pins) {
              const px = c.position.x + pin.position.x;
              const py = c.position.y + pin.position.y;
              for (let wi = 0; wi < wires.length; wi++) {
                const wire = wires[wi]!;
                if (Math.hypot(wire.start.x - px, wire.start.y - py) < 2) {
                  conns.push({ wireIdx: wi, endpoint: "start", pinOffset: pin.position });
                }
                if (Math.hypot(wire.end.x - px, wire.end.y - py) < 2) {
                  conns.push({ wireIdx: wi, endpoint: "end", pinOffset: pin.position });
                }
              }
            }
            setMoveConnections(conns);
            setMoveDrag({
              compIdx: ci,
              offset: { x: pos.x - c.position.x, y: pos.y - c.position.y },
            });
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            return;
          }
        }
      }
    },
    [zoom, pan, schTool, moveDrag, schematic],
  );

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      // Panning
      if (dragging && dragStart.current) {
        const dx = e.clientX - dragStart.current.x;
        const dy = e.clientY - dragStart.current.y;
        dragStart.current = { x: e.clientX, y: e.clientY };
        adjustPan(dx / zoom, dy / zoom);
        return;
      }

      if (!svgRef.current) return;
      const pos = screenToSvg(e, svgRef.current, zoom, pan);
      const components = schematic?.components ?? [];
      const snapped = schTool === "wire"
        ? snapToGridOrPin(pos, components, SCH_GRID)
        : { x: snapToGrid(pos.x, SCH_GRID), y: snapToGrid(pos.y, SCH_GRID) };

      // Update snap target indicator for wire mode
      if (schTool === "wire") {
        setSnapTarget(snapped as { x: number; y: number; isPin: boolean });
      } else {
        setSnapTarget(null);
      }

      // Move drag
      if (moveDrag) {
        const newPos = {
          x: snapToGrid(pos.x - moveDrag.offset.x, SCH_GRID),
          y: snapToGrid(pos.y - moveDrag.offset.y, SCH_GRID),
        };
        setGhostPos(newPos);
        return;
      }

      // Ghost position for placement/wire/label
      if (schTool === "place" || schTool === "wire" || schTool === "label") {
        setGhostPos(snapped);
      }

      // Wire preview
      if (schTool === "wire" && wireStart) {
        updateSchWirePreview(snapped);
      }
    },
    [dragging, zoom, adjustPan, pan, schTool, wireStart, updateSchWirePreview, moveDrag, schematic],
  );

  const onPointerUp = useCallback(
    () => {
      // Finish panning
      if (dragging) {
        setDragging(false);
        dragStart.current = null;
        return;
      }

      // Finish move drag with rubber-banding
      if (moveDrag && ghostPos) {
        if (moveConnections.length > 0) {
          const wireUpdates = moveConnections.map((conn) => ({
            wireIdx: conn.wireIdx,
            endpoint: conn.endpoint,
            pos: { x: ghostPos.x + conn.pinOffset.x, y: ghostPos.y + conn.pinOffset.y },
          }));
          moveSchematicComponentWithWires(moveDrag.compIdx, { x: ghostPos.x, y: ghostPos.y, z: 0 }, wireUpdates);
        } else {
          moveSchematicComponent(moveDrag.compIdx, { x: ghostPos.x, y: ghostPos.y, z: 0 });
        }
        setMoveDrag(null);
        setGhostPos(null);
        setMoveConnections([]);
        return;
      }
    },
    [dragging, moveDrag, ghostPos, moveConnections, moveSchematicComponent, moveSchematicComponentWithWires],
  );

  const onSvgClick = useCallback(
    (e: React.MouseEvent) => {
      if (!svgRef.current || dragging || moveDrag) return;
      const pos = screenToSvg(e, svgRef.current, zoom, pan);
      const components = schematic?.components ?? [];
      const snapped = schTool === "wire"
        ? snapToGridOrPin(pos, components, SCH_GRID)
        : { x: snapToGrid(pos.x, SCH_GRID), y: snapToGrid(pos.y, SCH_GRID) };

      // Place tool
      if (schTool === "place" && placingSymbol) {
        const sym = getSymbol(placingSymbol);
        if (!sym) return;
        const ref = nextRef(sym.prefix);
        const footprintTemplate = sym.footprintTemplate
          ? JSON.stringify(sym.footprintTemplate)
          : undefined;
        addSchematicComponent({
          ref,
          value: sym.defaultValue,
          footprintId: sym.footprintTemplate?.name ?? "",
          position: { x: snapped.x, y: snapped.y },
          rotation: placingRotation,
          pins: sym.pins.map((p) => ({ ...p })),
          properties: {
            symbolId: sym.id,
            ...(footprintTemplate ? { footprintTemplate } : {}),
          },
        });
        return;
      }

      // Wire tool
      if (schTool === "wire") {
        if (!wireStart) {
          startSchWire(snapped);
        } else {
          // Commit orthogonal wire segments (H then V) matching preview
          const mid = { x: snapped.x, y: wireStart.y };
          const newSegments: { start: { x: number; y: number }; end: { x: number; y: number } }[] = [];
          if (mid.x !== wireStart.x || mid.y !== wireStart.y) {
            addSchematicWire({ start: wireStart, end: mid });
            newSegments.push({ start: wireStart, end: mid });
          }
          if (snapped.x !== mid.x || snapped.y !== mid.y) {
            addSchematicWire({ start: mid, end: snapped });
            newSegments.push({ start: mid, end: snapped });
          }
          // Auto-junction detection
          const wires = schematic?.wires ?? [];
          const junctions = schematic?.junctions ?? [];
          const hasJunction = (p: { x: number; y: number }) =>
            junctions.some((j) => Math.hypot(j.position.x - p.x, j.position.y - p.y) < 1);
          const tryJunction = (p: { x: number; y: number }, segs: { start: { x: number; y: number }; end: { x: number; y: number } }[]) => {
            if (hasJunction(p)) return;
            for (const s of segs) {
              if (pointOnSegment(p, s.start, s.end)) {
                addSchematicJunction({ position: p });
                return;
              }
            }
          };
          // Check new wire endpoints against existing wires
          for (const seg of newSegments) {
            tryJunction(seg.start, wires);
            tryJunction(seg.end, wires);
          }
          // Check existing wire endpoints against new segments
          for (const w of wires) {
            tryJunction(w.start, newSegments);
            tryJunction(w.end, newSegments);
          }
          // Chain: new start = old end
          startSchWire(snapped);
        }
        return;
      }

      // Label tool
      if (schTool === "label") {
        const labelName = useElectronicsStore.getState().schLabelName;
        addSchematicLabel({
          name: labelName,
          position: snapped,
          scope: "Global",
        });
        return;
      }

      // Select tool: click on empty space deselects
      if (schTool === "select") {
        select({ type: "none" });
      }
    },
    [
      zoom, pan, schTool, placingSymbol, placingRotation, wireStart,
      addSchematicComponent, addSchematicWire, addSchematicLabel, addSchematicJunction,
      startSchWire, nextRef, select, dragging, moveDrag, schematic,
    ],
  );

  const onDblClick = useCallback(
    (e: React.MouseEvent) => {
      if (!svgRef.current || schTool !== "wire" || !wireStart) return;
      const pos = screenToSvg(e, svgRef.current, zoom, pan);
      const components = schematic?.components ?? [];
      const snapped = snapToGridOrPin(pos, components, SCH_GRID);
      // Commit segments
      const mid = { x: snapped.x, y: wireStart.y };
      const newSegments: { start: { x: number; y: number }; end: { x: number; y: number } }[] = [];
      if (mid.x !== wireStart.x || mid.y !== wireStart.y) {
        addSchematicWire({ start: wireStart, end: mid });
        newSegments.push({ start: wireStart, end: mid });
      }
      if (snapped.x !== mid.x || snapped.y !== mid.y) {
        addSchematicWire({ start: mid, end: snapped });
        newSegments.push({ start: mid, end: snapped });
      }
      // Auto-junction detection
      const wires = schematic?.wires ?? [];
      const junctions = schematic?.junctions ?? [];
      const hasJunction = (p: { x: number; y: number }) =>
        junctions.some((j) => Math.hypot(j.position.x - p.x, j.position.y - p.y) < 1);
      const tryJunction = (p: { x: number; y: number }, segs: { start: { x: number; y: number }; end: { x: number; y: number } }[]) => {
        if (hasJunction(p)) return;
        for (const s of segs) {
          if (pointOnSegment(p, s.start, s.end)) { addSchematicJunction({ position: p }); return; }
        }
      };
      for (const seg of newSegments) { tryJunction(seg.start, wires); tryJunction(seg.end, wires); }
      for (const w of wires) { tryJunction(w.start, newSegments); tryJunction(w.end, newSegments); }
      // End chain (don't start a new one)
      useElectronicsStore.getState().cancelSchWire();
    },
    [zoom, pan, schTool, wireStart, addSchematicWire, addSchematicJunction, schematic],
  );

  const onComponentClick = useCallback(
    (e: React.MouseEvent, idx: number, ref: string) => {
      // Wire tool: let the click fall through to the canvas handler so it snaps
      // to this component's pin — clicking a pin should start/finish a wire, not
      // select the part. (Without this, stopPropagation swallows the wire click.)
      if (schTool === "wire") return;
      e.stopPropagation();
      if (schTool === "delete") {
        removeSchematicComponent(idx);
      } else {
        select({ type: "component", ref });
      }
    },
    [schTool, select, removeSchematicComponent],
  );

  const onWireClick = useCallback(
    (e: React.MouseEvent, idx: number, net: string | null) => {
      e.stopPropagation();
      if (schTool === "delete") {
        removeSchematicWire(idx);
      } else if (net) {
        select({ type: "net", netId: net });
      }
    },
    [schTool, select, removeSchematicWire],
  );

  const onLabelClick = useCallback(
    (e: React.MouseEvent, idx: number, name: string) => {
      e.stopPropagation();
      if (schTool === "delete") {
        removeSchematicLabel(idx);
      } else {
        select({ type: "net", netId: name });
      }
    },
    [schTool, select, removeSchematicLabel],
  );

  if (!schematic) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3">
        <span className="text-text-muted text-sm">No schematic data</span>
        <button
          className="px-3 py-1.5 text-xs bg-brand text-white rounded hover:bg-brand/90 transition-colors"
          onClick={() => {
            initSchematic();
            const doc = useDocumentStore.getState().document;
            if (getPcbNodeIds(doc).length === 0) initPcb();
          }}
        >
          New Circuit
        </button>
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
    ghost: "rgba(59, 130, 246, 0.5)",
  };

  const cursorStyle = (() => {
    if (dragging) return "grabbing";
    if (schTool === "place") return "copy";
    if (schTool === "wire") return "crosshair";
    if (schTool === "delete") return "not-allowed";
    if (schTool === "move") return "move";
    return "default";
  })();

  // Ghost symbol for placement preview
  const ghostSymbol = placingSymbol ? getSymbol(placingSymbol) : null;

  return (
    <svg
      ref={svgRef}
      className="w-full h-full"
      style={{ background: colors.bg, cursor: cursorStyle }}
      onWheel={onWheel}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onClick={onSvgClick}
      onDoubleClick={onDblClick}
      onContextMenu={(e) => {
        e.preventDefault();
        if (wireStart) {
          useElectronicsStore.getState().cancelSchWire();
        }
      }}
    >
      <g transform={`translate(${pan.x * zoom + 200}, ${pan.y * zoom + 200}) scale(${zoom})`}>
        {/* Grid */}
        <defs>
          <pattern id="sch-grid" width={SCH_GRID} height={SCH_GRID} patternUnits="userSpaceOnUse">
            <circle cx={SCH_GRID / 2} cy={SCH_GRID / 2} r={0.5} fill={isDark ? "#222" : "#ddd"} />
          </pattern>
          <style>{`
            @keyframes pin-pulse {
              0%, 100% { r: 7; opacity: 0.3; }
              50% { r: 10; opacity: 0.15; }
            }
            .sch-pin-glow { animation: pin-pulse 1.2s ease-in-out infinite; }
          `}</style>
        </defs>
        <rect x={-1000} y={-1000} width={2000} height={2000} fill="url(#sch-grid)" />

        {/* Wires */}
        {schematic.wires.map((wire, i) => {
          const net = getNetForWire(wire, netlist, schematic.components);
          const highlight = isNetActive(net);
          const overrides = wireOverrides.get(i);
          const s = overrides?.start ?? wire.start;
          const en = overrides?.end ?? wire.end;
          const v = netVoltage(net);
          const simStroke = v != null ? voltageColor(v) : null;
          return (
            <line
              key={`w-${i}`}
              x1={s.x}
              y1={s.y}
              x2={en.x}
              y2={en.y}
              stroke={simStroke ?? (highlight ? colors.accent : colors.wire)}
              strokeWidth={simStroke != null ? 2.5 : highlight ? 2 : 1.5}
              strokeLinecap="round"
              style={
                simStroke != null
                  ? { filter: `drop-shadow(0 0 3px ${simStroke})` }
                  : highlight
                    ? { filter: `drop-shadow(0 0 4px ${colors.accentGlow})` }
                    : undefined
              }
              className="cursor-pointer"
              onClick={(e) => onWireClick(e, i, net)}
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

        {/* Wire-pin connection dots */}
        {connectionDots.map((d, i) => (
          <circle key={`cd-${i}`} cx={d.x} cy={d.y} r={3} fill={colors.wire} />
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
              onClick={(e) => onLabelClick(e, i, label.name)}
              onPointerEnter={() => setHoveredNet(label.name)}
              onPointerLeave={() => setHoveredNet(null)}
            >
              {label.name}
            </text>
          </g>
        ))}

        {/* Components */}
        {schematic.components.map((comp, i) => {
          const isSelected = activeComponentRef === comp.ref;
          const compNets = getNetsForComponent(comp.ref, netlist);
          const hasActiveNet = [...compNets].some((n) => isNetActive(n));
          const highlighted = isSelected || hasActiveNet;

          // Check if this component has a known symbol
          const sym = comp.properties?.symbolId ? getSymbol(comp.properties.symbolId) : null;

          // If being moved, show at ghost position
          const displayPos = (moveDrag?.compIdx === i && ghostPos)
            ? ghostPos
            : comp.position;

          if (sym) {
            // Render with symbol graphics
            return (
              <g
                key={`c-${i}`}
                transform={`translate(${displayPos.x}, ${displayPos.y})${comp.rotation ? ` rotate(${comp.rotation})` : ""}`}
                className="cursor-pointer"
                onClick={(e) => onComponentClick(e, i, comp.ref)}
                onContextMenu={(e) => onComponentContextMenu(e, comp.ref)}
                onPointerEnter={() => {
                  const nets = getNetsForComponent(comp.ref, netlist);
                  const first = nets.values().next().value;
                  if (first) setHoveredNet(first);
                }}
                onPointerLeave={() => setHoveredNet(null)}
                opacity={moveDrag?.compIdx === i ? 0.6 : 1}
              >
                {/* LED glow — brightness ∝ current (the circuit "comes alive") */}
                {simOn &&
                  comp.properties?.symbolId === "led" &&
                  (() => {
                    const b = Math.max(0, Math.min(1, Math.abs(compCurrent(comp.ref) ?? 0) / 0.01));
                    if (b <= 0.03) return null;
                    return (
                      <circle
                        cx={20}
                        cy={15}
                        r={9 + 13 * b}
                        fill="#ff5a36"
                        opacity={0.55 * b}
                        style={{ filter: `blur(${4 + 6 * b}px)` }}
                      />
                    );
                  })()}
                {/* Motor rotor — a spoke that spins at the rotor angle. */}
                {simOn &&
                  comp.properties?.symbolId === "motor" &&
                  (() => {
                    const deg = ((motorAngle(comp.ref) * 180) / Math.PI) % 360;
                    const spinning = Math.abs(motorAngle(comp.ref)) > 0.01;
                    return (
                      <g transform={`rotate(${deg} 20 15)`}>
                        <line
                          x1={20}
                          y1={3}
                          x2={20}
                          y2={27}
                          stroke={spinning ? "#4ade80" : colors.bodyStroke}
                          strokeWidth={2.5}
                          strokeLinecap="round"
                        />
                        <circle cx={20} cy={3} r={2.5} fill="#4ade80" opacity={spinning ? 1 : 0.3} />
                      </g>
                    );
                  })()}
                {renderSymbolGraphics(
                  sym.graphics,
                  highlighted ? colors.accent : colors.bodyStroke,
                  colors.body,
                )}
                {/* Pin endpoints */}
                {comp.pins.map((pin, pi) => {
                  const pinNet = getNetForPin(comp.ref, pin.number, netlist);
                  const pinHighlight = isNetActive(pinNet);
                  return (
                    <circle
                      key={`p-${pi}`}
                      cx={pin.position.x}
                      cy={pin.position.y}
                      r={2}
                      fill={pinHighlight ? colors.accent : colors.bodyStroke}
                    />
                  );
                })}
                {/* Reference */}
                <text
                  x={20}
                  y={-6}
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
                  x={20}
                  y={-6 + (sym.graphics.find((g) => g.type === "rect")?.height ?? 30) + 16}
                  fontSize={8}
                  fill={colors.textMuted}
                  textAnchor="middle"
                  fontFamily="monospace"
                >
                  {comp.value}
                </text>
              </g>
            );
          }

          // Fallback: generic rectangle rendering
          const h = Math.max(COMPONENT_HEIGHT, comp.pins.length * 8);

          return (
            <g
              key={`c-${i}`}
              transform={`translate(${displayPos.x}, ${displayPos.y})${comp.rotation ? ` rotate(${comp.rotation})` : ""}`}
              className="cursor-pointer"
              onClick={(e) => onComponentClick(e, i, comp.ref)}
              onContextMenu={(e) => onComponentContextMenu(e, comp.ref)}
              onPointerEnter={() => {
                const nets = getNetsForComponent(comp.ref, netlist);
                const first = nets.values().next().value;
                if (first) setHoveredNet(first);
              }}
              onPointerLeave={() => setHoveredNet(null)}
              opacity={moveDrag?.compIdx === i ? 0.6 : 1}
            >
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

        {/* DC operating-point annotations (Analyze flow): node voltages on
            nets, device currents under components. */}
        {dcOn && (
          <g pointerEvents="none">
            {dcNetAnchors.map((a) => (
              <g key={`dcv-${a.net}`} transform={`translate(${a.x}, ${a.y - 6})`}>
                <rect
                  x={-2}
                  y={-9}
                  width={fmtEng(a.v, "V").length * 5.4 + 4}
                  height={12}
                  fill={isDark ? "#052e16" : "#dcfce7"}
                  opacity={0.9}
                  rx={2}
                />
                <text fontSize={8} fill="#4ade80" fontFamily="monospace" dominantBaseline="hanging" y={-8}>
                  {fmtEng(a.v, "V")}
                </text>
              </g>
            ))}
            {schematic.components.map((comp, i) => {
              const cur = dcCompCurrent(comp.ref);
              if (cur == null) return null;
              return (
                <text
                  key={`dci-${i}`}
                  x={comp.position.x + 20}
                  y={comp.position.y + 48}
                  fontSize={7}
                  fill="#4ade80"
                  textAnchor="middle"
                  fontFamily="monospace"
                  opacity={0.9}
                >
                  {fmtEng(cur, "A")}
                </text>
              );
            })}
          </g>
        )}

        {/* Fail-closed blockers: pin each unmappable component with a red
            outline (the panel lists the reasons). */}
        {analysis.status === "blocked" &&
          schematic.components.map((comp, i) => {
            if (!blockedRefs.has(comp.ref)) return null;
            return (
              <g key={`blk-${i}`} pointerEvents="none">
                <rect
                  x={comp.position.x - 8}
                  y={comp.position.y - 16}
                  width={56}
                  height={60}
                  fill="none"
                  stroke="#f87171"
                  strokeWidth={1.5}
                  strokeDasharray="4 3"
                  rx={4}
                  style={{ filter: "drop-shadow(0 0 4px rgba(248,113,113,0.5))" }}
                />
                <text
                  x={comp.position.x + 44}
                  y={comp.position.y - 8}
                  fontSize={10}
                  fill="#f87171"
                  fontFamily="monospace"
                >
                  !
                </text>
                <title>{blockerMessage(comp.ref)}</title>
              </g>
            );
          })}

        {/* Ghost component preview (place tool) */}
        {schTool === "place" && ghostSymbol && ghostPos && (
          <g
            transform={`translate(${ghostPos.x}, ${ghostPos.y})${placingRotation ? ` rotate(${placingRotation})` : ""}`}
            opacity={0.5}
            pointerEvents="none"
          >
            {renderSymbolGraphics(ghostSymbol.graphics, colors.accent, "none")}
            {ghostSymbol.pins.map((pin, pi) => (
              <circle
                key={`gp-${pi}`}
                cx={pin.position.x}
                cy={pin.position.y}
                r={2}
                fill={colors.accent}
                opacity={0.5}
              />
            ))}
          </g>
        )}

        {/* Wire preview */}
        {schTool === "wire" && wireStart && wirePreview && (() => {
          const dx = Math.abs(wirePreview.x - wireStart.x);
          const dy = Math.abs(wirePreview.y - wireStart.y);
          const cornerX = wirePreview.x;
          const cornerY = wireStart.y;
          const r = Math.min(4, dx, dy);
          // Build path: H segment → rounded Q corner → V segment
          let d: string;
          if (dx < 0.5 || dy < 0.5) {
            // Straight line (pure H or V)
            d = `M${wireStart.x},${wireStart.y} L${wirePreview.x},${wirePreview.y}`;
          } else {
            const sx = wirePreview.x > wireStart.x ? 1 : -1;
            const sy = wirePreview.y > wireStart.y ? 1 : -1;
            d = `M${wireStart.x},${wireStart.y} L${cornerX - r * sx},${cornerY} Q${cornerX},${cornerY} ${cornerX},${cornerY + r * sy} L${wirePreview.x},${wirePreview.y}`;
          }
          return (
            <g pointerEvents="none">
              <path
                d={d}
                fill="none"
                stroke={colors.accent}
                strokeWidth={2}
                strokeLinecap="round"
                opacity={0.6}
                strokeDasharray="4 2"
              />
              <circle cx={wireStart.x} cy={wireStart.y} r={3} fill={colors.accent} opacity={0.8} />
              <circle cx={wirePreview.x} cy={wirePreview.y} r={3} fill={colors.accent} opacity={0.8} />
            </g>
          );
        })()}

        {/* Pin dots during wire mode — show all available targets */}
        {schTool === "wire" && schematic.components.map((comp, ci) =>
          comp.pins.map((pin, pi) => (
            <circle
              key={`pd-${ci}-${pi}`}
              cx={comp.position.x + pin.position.x}
              cy={comp.position.y + pin.position.y}
              r={2}
              fill={colors.accent}
              opacity={0.3}
              pointerEvents="none"
            />
          ))
        )}

        {/* Snap target indicator */}
        {schTool === "wire" && snapTarget && (
          <g pointerEvents="none">
            {snapTarget.isPin ? (
              <>
                <circle cx={snapTarget.x} cy={snapTarget.y} r={8} fill="none" stroke={colors.accent} strokeWidth={1.5} className="sch-pin-glow" />
                <circle cx={snapTarget.x} cy={snapTarget.y} r={4} fill={colors.accent} opacity={0.6} />
              </>
            ) : (
              <circle cx={snapTarget.x} cy={snapTarget.y} r={2.5} fill="none" stroke={colors.accent} strokeWidth={1} opacity={0.4} />
            )}
          </g>
        )}

        {/* Label ghost */}
        {schTool === "label" && ghostPos && (
          <g transform={`translate(${ghostPos.x}, ${ghostPos.y})`} opacity={0.5} pointerEvents="none">
            <rect
              x={-2}
              y={-10}
              width={useElectronicsStore.getState().schLabelName.length * 6 + 4}
              height={14}
              fill={colors.accent}
              opacity={0.3}
              rx={2}
            />
            <text
              fontSize={9}
              fill={colors.accent}
              fontFamily="monospace"
              dominantBaseline="middle"
            >
              {useElectronicsStore.getState().schLabelName}
            </text>
          </g>
        )}
      </g>
    </svg>
  );
}
