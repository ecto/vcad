/**
 * InstancedMesh renderer for PCB vias.
 *
 * Outer ring as cylinders + drill hole as dark disc.
 * Net highlighting via per-instance color.
 */

import { useRef, useEffect } from "react";
import * as THREE from "three";
import type { Pcb } from "@vcad/ir";
import { layerZ } from "./pcb-geometry";

interface Props {
  pcb: Pcb;
  activeNet: string | null;
  hoveredNet: string | null;
  explosion: number;
}

const ACCENT_COLOR = new THREE.Color("#3b82f6");
const VIA_COLOR = new THREE.Color("#888888");
const DRILL_COLOR = new THREE.Color("#111111");
const TEMP_MATRIX = new THREE.Matrix4();
const TEMP_POS = new THREE.Vector3();
const TEMP_SCALE = new THREE.Vector3();
const TEMP_QUAT = new THREE.Quaternion();

export function PcbViaMesh({ pcb, activeNet, hoveredNet, explosion }: Props) {
  const outerRef = useRef<THREE.InstancedMesh>(null);
  const drillRef = useRef<THREE.InstancedMesh>(null);

  const vias = pcb.vias;
  const thickness = pcb.outline.thickness;

  useEffect(() => {
    const outer = outerRef.current;
    const drill = drillRef.current;
    if (!outer || vias.length === 0) return;

    const z = layerZ("FCu", thickness, explosion);

    for (let i = 0; i < vias.length; i++) {
      const via = vias[i]!;
      const isActive = via.net === activeNet || via.net === hoveredNet;

      // Outer cylinder (flat disc)
      TEMP_POS.set(via.position.x, via.position.y, z);
      TEMP_SCALE.set(via.diameter, 0.07, via.diameter);
      TEMP_QUAT.identity();
      TEMP_MATRIX.compose(TEMP_POS, TEMP_QUAT, TEMP_SCALE);
      outer.setMatrixAt(i, TEMP_MATRIX);
      outer.setColorAt(i, isActive ? ACCENT_COLOR : VIA_COLOR);

      // Drill hole
      if (drill) {
        TEMP_POS.set(via.position.x, via.position.y, z + 0.01);
        TEMP_SCALE.set(via.drill, 0.07, via.drill);
        TEMP_MATRIX.compose(TEMP_POS, TEMP_QUAT, TEMP_SCALE);
        drill.setMatrixAt(i, TEMP_MATRIX);
        drill.setColorAt(i, DRILL_COLOR);
      }
    }

    outer.count = vias.length;
    outer.instanceMatrix.needsUpdate = true;
    if (outer.instanceColor) outer.instanceColor.needsUpdate = true;

    if (drill) {
      drill.count = vias.length;
      drill.instanceMatrix.needsUpdate = true;
      if (drill.instanceColor) drill.instanceColor.needsUpdate = true;
    }
  }, [vias, activeNet, hoveredNet, thickness, explosion]);

  if (vias.length === 0) return null;

  return (
    <>
      <instancedMesh
        ref={outerRef}
        args={[undefined, undefined, Math.max(vias.length, 1)]}
        frustumCulled={false}
      >
        <cylinderGeometry args={[0.5, 0.5, 1, 16]} />
        <meshStandardMaterial vertexColors roughness={0.32} metalness={0.4} envMapIntensity={1.5} />
      </instancedMesh>
      <instancedMesh
        ref={drillRef}
        args={[undefined, undefined, Math.max(vias.length, 1)]}
        frustumCulled={false}
      >
        <cylinderGeometry args={[0.5, 0.5, 1, 16]} />
        <meshStandardMaterial vertexColors roughness={0.9} metalness={0} />
      </instancedMesh>
    </>
  );
}
