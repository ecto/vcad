/**
 * Extruded board outline mesh.
 *
 * FR4 green slab from board outline vertices + cutout holes.
 * Approach reused from Pcb3dPreview.tsx:PcbMesh.
 */

import { useMemo } from "react";
import * as THREE from "three";
import type { Pcb } from "@vcad/ir";
import { layerZOffset } from "./pcb-geometry";

interface Props {
  pcb: Pcb;
  explosion: number;
}

/** Dielectric layer definitions between copper layers. */
const DIELECTRIC_LAYERS = [
  { name: "Core 1", top: "FCu", bottom: "In1Cu" },
  { name: "Core 2", top: "In1Cu", bottom: "In2Cu" },
  { name: "Core 3", top: "In2Cu", bottom: "BCu" },
] as const;

export function PcbBoardMesh({ pcb, explosion }: Props) {
  const boardShape = useMemo(() => {
    const verts = pcb.outline.vertices;
    if (verts.length < 3) return null;

    const shape = new THREE.Shape();
    shape.moveTo(verts[0]!.x, verts[0]!.y);
    for (let i = 1; i < verts.length; i++) {
      shape.lineTo(verts[i]!.x, verts[i]!.y);
    }
    shape.closePath();

    // Add cutout holes
    if (pcb.outline.cutouts) {
      for (const cutout of pcb.outline.cutouts) {
        if (cutout.length < 3) continue;
        const hole = new THREE.Path();
        hole.moveTo(cutout[0]!.x, cutout[0]!.y);
        for (let i = 1; i < cutout.length; i++) {
          hole.lineTo(cutout[i]!.x, cutout[i]!.y);
        }
        hole.closePath();
        shape.holes.push(hole);
      }
    }

    return shape;
  }, [pcb.outline]);

  const thickness = pcb.outline.thickness;

  const mainGeometry = useMemo(() => {
    if (!boardShape) return null;
    return new THREE.ExtrudeGeometry(boardShape, {
      depth: thickness,
      bevelEnabled: false,
    });
  }, [boardShape, thickness]);

  // Thin slab geometry for dielectric layers when exploded
  const slabGeometry = useMemo(() => {
    if (!boardShape || explosion <= 0) return null;
    return new THREE.ExtrudeGeometry(boardShape, {
      depth: 0.1,
      bevelEnabled: false,
    });
  }, [boardShape, explosion]);

  if (!mainGeometry) return null;

  return (
    <group>
      {/* Main board slab */}
      <mesh geometry={mainGeometry} position={[0, 0, -thickness / 2]}>
        <meshStandardMaterial
          color="#0d5a2d"
          roughness={0.8}
          metalness={0}
          side={THREE.DoubleSide}
        />
      </mesh>

      {/* Dielectric slabs between copper layers (visible when exploded) */}
      {explosion > 0 && slabGeometry && DIELECTRIC_LAYERS.map((dl) => {
        const topZ = layerZOffset(dl.top as any, explosion);
        const bottomZ = layerZOffset(dl.bottom as any, explosion);
        const midZ = (topZ + bottomZ) / 2;
        return (
          <mesh
            key={dl.name}
            geometry={slabGeometry}
            position={[0, 0, midZ - 0.05]}
          >
            <meshStandardMaterial
              color="#1a7a3a"
              roughness={0.9}
              metalness={0}
              transparent
              opacity={0.3}
              side={THREE.DoubleSide}
            />
          </mesh>
        );
      })}
    </group>
  );
}
