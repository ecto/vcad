/**
 * PcbScene: assembles all PCB geometry children inside the rotation group.
 *
 * Mounts board, traces, pads, vias, footprints, ratsnest, route preview,
 * DRC markers, grid, and an invisible interaction plane.
 */

import { useCallback, useMemo, useRef } from "react";
import { Grid, Plane } from "@react-three/drei";
import { useThree, type ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import type { Vec2 } from "@vcad/ir";
import { useElectronicsStore } from "@/stores/electronics-store";
import { worldToPcb, layerZ } from "./pcb-geometry";

import { PcbKernelBoard } from "./PcbKernelBoard";
import { PcbFootprint3D } from "./PcbFootprint3D";
import { PcbComponentBodies3D } from "./PcbComponentBodies3D";
import { PcbRatsnest3D } from "./PcbRatsnest3D";
import { PcbRoutePreview3D } from "./PcbRoutePreview3D";
import { PcbDrcMarkers3D } from "./PcbDrcMarkers3D";

export function PcbScene({ showBoard = true }: { showBoard?: boolean } = {}) {
  const { invalidate } = useThree();
  const planeRef = useRef<THREE.Mesh>(null);
  const groupRef = useRef<THREE.Group>(null);

  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;

  const selection = useElectronicsStore((s) => s.selection);
  const hoveredNet = useElectronicsStore((s) => s.hoveredNet);
  const netlist = useElectronicsStore((s) => s.netlist);
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const pcbGridSize = useElectronicsStore((s) => s.pcbGridSize);
  const pcbSnapToGrid = useElectronicsStore((s) => s.pcbSnapToGrid);
  const pcbActiveLayer = useElectronicsStore((s) => s.pcbActiveLayer);
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const routeActive = useElectronicsStore((s) => s.routeActive);
  const routePreview = useElectronicsStore((s) => s.routePreview);
  const routeStartPad = useElectronicsStore((s) => s.routeStartPad);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const pcbDragging = useElectronicsStore((s) => s.pcbDragging);
  const stackupExplosion = useElectronicsStore((s) => s.stackupExplosion);
  const interferingFootprints = useElectronicsStore((s) => s.interferingFootprints);

  const select = useElectronicsStore((s) => s.select);
  const setHoveredNet = useElectronicsStore((s) => s.setHoveredNet);
  const startRouteFromRatsnest = useElectronicsStore((s) => s.startRouteFromRatsnest);
  const updateRoutePreview = useElectronicsStore((s) => s.updateRoutePreview);
  const startPcbDrag = useElectronicsStore((s) => s.startPcbDrag);
  const cancelPcbDrag = useElectronicsStore((s) => s.cancelPcbDrag);
  const moveFootprint = useDocumentStore((s) => s.moveFootprint);
  const removeTrace = useDocumentStore((s) => s.removeTrace);
  const removeVia = useDocumentStore((s) => s.removeVia);

  // Active net from selection
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

  // Interaction plane pointer handlers
  const dragRef = useRef<{ fpIdx: number; startWorld: Vec2 } | null>(null);

  const onPlanePointerDown = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (!pcb || e.button !== 0) return;
      e.stopPropagation();

      // Convert the world-space ray/plane intersection into the board's local
      // (kernel) frame so picking is correct from ANY camera angle — the
      // interaction plane is coplanar with the board, so the ray lands where
      // the geometry actually is.
      const world = e.point as THREE.Vector3;
      const point = groupRef.current ? groupRef.current.worldToLocal(world.clone()) : world;
      const pcbPos = worldToPcb(point, pcbGridSize, pcbSnapToGrid);

      // Move tool: start footprint drag
      if (pcbTool === "move") {
        for (let i = pcb.footprints.length - 1; i >= 0; i--) {
          const fp = pcb.footprints[i]!;
          const halfW = 5, halfH = 5;
          if (
            pcbPos.x >= fp.position.x - halfW &&
            pcbPos.x <= fp.position.x + halfW &&
            pcbPos.y >= fp.position.y - halfH &&
            pcbPos.y <= fp.position.y + halfH
          ) {
            startPcbDrag(i, fp.position);
            dragRef.current = { fpIdx: i, startWorld: pcbPos };
            return;
          }
        }
      }

      // Select tool: hit-test elements
      if (pcbTool === "select") {
        // Hit-test footprints
        for (let i = pcb.footprints.length - 1; i >= 0; i--) {
          const fp = pcb.footprints[i]!;
          for (const pad of fp.pads) {
            const px = fp.position.x + pad.position.x;
            const py = fp.position.y + pad.position.y;
            const dist = Math.sqrt((pcbPos.x - px) ** 2 + (pcbPos.y - py) ** 2);
            if (dist < 1.5) {
              select({ type: "pad", fpRef: fp.ref, padNum: pad.number, net: pad.net ?? "" });
              return;
            }
          }
          const halfW = 5, halfH = 5;
          if (
            pcbPos.x >= fp.position.x - halfW &&
            pcbPos.x <= fp.position.x + halfW &&
            pcbPos.y >= fp.position.y - halfH &&
            pcbPos.y <= fp.position.y + halfH
          ) {
            select({ type: "footprint", ref: fp.ref });
            return;
          }
        }

        // Hit-test traces
        for (let i = 0; i < pcb.traces.length; i++) {
          const trace = pcb.traces[i]!;
          // Point-to-segment distance
          const dx = trace.end.x - trace.start.x;
          const dy = trace.end.y - trace.start.y;
          const len2 = dx * dx + dy * dy;
          if (len2 < 1e-6) continue;
          let t = ((pcbPos.x - trace.start.x) * dx + (pcbPos.y - trace.start.y) * dy) / len2;
          t = Math.max(0, Math.min(1, t));
          const closestX = trace.start.x + t * dx;
          const closestY = trace.start.y + t * dy;
          const dist = Math.sqrt((pcbPos.x - closestX) ** 2 + (pcbPos.y - closestY) ** 2);
          if (dist < trace.width / 2 + 0.5) {
            select({ type: "trace", idx: i, net: trace.net });
            return;
          }
        }

        // Hit-test vias
        for (let i = 0; i < pcb.vias.length; i++) {
          const via = pcb.vias[i]!;
          const dist = Math.sqrt(
            (pcbPos.x - via.position.x) ** 2 + (pcbPos.y - via.position.y) ** 2,
          );
          if (dist < via.diameter / 2 + 0.3) {
            select({ type: "via", idx: i, net: via.net });
            return;
          }
        }

        // Clicked empty space
        select({ type: "none" });
      }

      // Delete tool
      if (pcbTool === "delete") {
        // Hit-test traces/vias for deletion
        for (let i = 0; i < pcb.traces.length; i++) {
          const trace = pcb.traces[i]!;
          const dx = trace.end.x - trace.start.x;
          const dy = trace.end.y - trace.start.y;
          const len2 = dx * dx + dy * dy;
          if (len2 < 1e-6) continue;
          let t = ((pcbPos.x - trace.start.x) * dx + (pcbPos.y - trace.start.y) * dy) / len2;
          t = Math.max(0, Math.min(1, t));
          const closestX = trace.start.x + t * dx;
          const closestY = trace.start.y + t * dy;
          const dist = Math.sqrt((pcbPos.x - closestX) ** 2 + (pcbPos.y - closestY) ** 2);
          if (dist < trace.width / 2 + 0.5 && activeBoardNodeId != null) {
            removeTrace(activeBoardNodeId, i);
            return;
          }
        }
        for (let i = 0; i < pcb.vias.length; i++) {
          const via = pcb.vias[i]!;
          const dist = Math.sqrt(
            (pcbPos.x - via.position.x) ** 2 + (pcbPos.y - via.position.y) ** 2,
          );
          if (dist < via.diameter / 2 + 0.3 && activeBoardNodeId != null) {
            removeVia(activeBoardNodeId, i);
            return;
          }
        }
      }

      // Route tool: start from pad
      if (pcbTool === "route") {
        for (const fp of pcb.footprints) {
          for (const pad of fp.pads) {
            const px = fp.position.x + pad.position.x;
            const py = fp.position.y + pad.position.y;
            const dist = Math.sqrt((pcbPos.x - px) ** 2 + (pcbPos.y - py) ** 2);
            if (dist < 1.5) {
              useElectronicsStore.getState().startRoute(fp.ref, pad.number, pad.net ?? "");
              return;
            }
          }
        }
      }
    },
    [pcb, pcbTool, pcbGridSize, pcbSnapToGrid, activeBoardNodeId, select, startPcbDrag, removeTrace, removeVia],
  );

  const onPlanePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (!pcb) return;
      const world = e.point as THREE.Vector3;
      const point = groupRef.current ? groupRef.current.worldToLocal(world.clone()) : world;
      const pcbPos = worldToPcb(point, pcbGridSize, pcbSnapToGrid);

      // Footprint drag
      if (pcbDragging && activeBoardNodeId != null) {
        moveFootprint(activeBoardNodeId, pcbDragging.fpIdx, { x: pcbPos.x, y: pcbPos.y, z: 0 });
        invalidate();
        return;
      }

      // Route preview
      if (routeActive) {
        updateRoutePreview([pcbPos]);
        invalidate();
      }
    },
    [pcb, pcbDragging, routeActive, pcbGridSize, pcbSnapToGrid, activeBoardNodeId, moveFootprint, updateRoutePreview, invalidate],
  );

  const onPlanePointerUp = useCallback(() => {
    if (pcbDragging) {
      cancelPcbDrag();
    }
  }, [pcbDragging, cancelPcbDrag]);

  if (!pcb) return null;

  const boardThickness = pcb.outline.thickness;
  // Board outline center — anchors the interaction plane + grid on the board
  // (was previously hardcoded to a 50x30 board's center).
  let bMinX = Infinity, bMaxX = -Infinity, bMinY = Infinity, bMaxY = -Infinity;
  for (const v of pcb.outline.vertices) {
    if (v.x < bMinX) bMinX = v.x;
    if (v.x > bMaxX) bMaxX = v.x;
    if (v.y < bMinY) bMinY = v.y;
    if (v.y > bMaxY) bMaxY = v.y;
  }
  const boardCx = isFinite(bMinX) ? (bMinX + bMaxX) / 2 : 0;
  const boardCy = isFinite(bMinY) ? (bMinY + bMaxY) / 2 : 0;
  const surfaceZ = layerZ("FCu", boardThickness);

  return (
    <group ref={groupRef}>
      {/* Grid (infinite fading dots) */}
      <Grid
        position={[boardCx, boardCy, surfaceZ - 0.05]}
        rotation={[Math.PI / 2, 0, 0]}
        args={[200, 200]}
        cellSize={pcbGridSize}
        cellColor="#333333"
        sectionSize={pcbGridSize * 10}
        sectionColor="#444444"
        fadeDistance={100}
        infiniteGrid
      />

      {/* Invisible interaction plane, coplanar with the board surface (kernel
          XY at z=surfaceZ, normal +Z) so cursor raycasts map to the right
          board coords from any camera angle. PlaneGeometry already lies in
          local XY, so no rotation. */}
      <Plane
        ref={planeRef}
        args={[500, 500]}
        position={[boardCx, boardCy, surfaceZ]}
        visible={false}
        onPointerDown={onPlanePointerDown}
        onPointerMove={onPlanePointerMove}
        onPointerUp={onPlanePointerUp}
      />

      {/* Board + copper — unified kernel meshes (same source as the MCP
          viewer): laminate, translucent soldermask shells, raw copper under
          the mask, exposed ENIG pads/vias. Selection/hover highlights are
          index-subset overlays on the kernel entity ranges. showBoard=false
          suppresses the board-body roles when the board's kernel part
          already renders in the main scene (avoids a duplicate slab). */}
      <PcbKernelBoard
        pcb={pcb}
        layers={pcbLayers}
        activeNet={activeNet}
        hoveredNet={hoveredNet}
        selection={selection}
        explosion={stackupExplosion}
        showBoard={showBoard}
      />

      {/* Footprint graphics (silkscreen, courtyard, ref text) */}
      {pcb.footprints.map((fp, i) => (
        <PcbFootprint3D
          key={`fp-${i}`}
          footprint={fp}
          layers={pcbLayers}
          boardThickness={boardThickness}
          highlight={activeFootprintRef === fp.ref}
          interfering={interferingFootprints.includes(fp.ref)}
          explosion={stackupExplosion}
        />
      ))}

      {/* Height-aware 3D component bodies (populated-board view). Clashing
          components (vs the surrounding enclosure) render in a warning tint. */}
      <PcbComponentBodies3D />

      {/* Ratsnest */}
      <PcbRatsnest3D
        pcb={pcb}
        netlist={netlist}
        activeNet={activeNet}
        hoveredNet={hoveredNet}
        boardThickness={boardThickness}
        explosion={stackupExplosion}
        onStartRoute={startRouteFromRatsnest}
        onHoverNet={setHoveredNet}
      />

      {/* Route preview */}
      {routeActive && (
        <PcbRoutePreview3D
          pcb={pcb}
          routeStartPad={routeStartPad}
          routePreview={routePreview}
          boardThickness={boardThickness}
          activeLayer={pcbActiveLayer}
          explosion={stackupExplosion}
        />
      )}

      {/* DRC violation markers */}
      <PcbDrcMarkers3D
        violations={drcViolations}
        boardThickness={boardThickness}
        explosion={stackupExplosion}
      />
    </group>
  );
}
