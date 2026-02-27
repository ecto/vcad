/**
 * InstancedMesh renderer for all PCB trace segments.
 *
 * Uses a unit box scaled/rotated per trace. Color from layer config.
 * Active net gets emissive highlight.
 */

import { useRef, useMemo, useEffect } from "react";
import * as THREE from "three";
import type { Pcb } from "@vcad/ir";
import { buildTraceMatrix, getLayerColor, isLayerVisible } from "./pcb-geometry";
import type { LayerConfig } from "@/stores/electronics-store";

interface Props {
  pcb: Pcb;
  layers: LayerConfig[];
  activeNet: string | null;
  hoveredNet: string | null;
  explosion: number;
}

const ACCENT_COLOR = new THREE.Color("#3b82f6");
const TEMP_MATRIX = new THREE.Matrix4();
const TEMP_COLOR = new THREE.Color();

export function PcbTraceMesh({ pcb, layers, activeNet, hoveredNet, explosion }: Props) {
  const meshRef = useRef<THREE.InstancedMesh>(null);

  // Filter to visible traces
  const visibleTraces = useMemo(() => {
    return pcb.traces.filter((t) => isLayerVisible(layers, t.layer));
  }, [pcb.traces, layers]);

  const count = visibleTraces.length;

  // Update instance matrices and colors
  useEffect(() => {
    const mesh = meshRef.current;
    if (!mesh || count === 0) return;

    const thickness = pcb.outline.thickness;

    for (let i = 0; i < count; i++) {
      const trace = visibleTraces[i]!;
      buildTraceMatrix(trace, thickness, explosion, TEMP_MATRIX);
      mesh.setMatrixAt(i, TEMP_MATRIX);

      const isActive =
        trace.net === activeNet || trace.net === hoveredNet;
      if (isActive) {
        mesh.setColorAt(i, ACCENT_COLOR);
      } else {
        TEMP_COLOR.set(getLayerColor(layers, trace.layer));
        mesh.setColorAt(i, TEMP_COLOR);
      }
    }

    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
    mesh.count = count;
  }, [visibleTraces, count, pcb.outline.thickness, layers, activeNet, hoveredNet, explosion]);

  if (count === 0) return null;

  return (
    <instancedMesh
      ref={meshRef}
      args={[undefined, undefined, Math.max(count, 1)]}
      frustumCulled={false}
    >
      <boxGeometry args={[1, 1, 1]} />
      <meshStandardMaterial
        vertexColors
        roughness={0.4}
        metalness={0.6}
        side={THREE.DoubleSide}
      />
    </instancedMesh>
  );
}
