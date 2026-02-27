/**
 * Extruded board outline mesh.
 *
 * FR4 green slab from board outline vertices + cutout holes.
 * Approach reused from Pcb3dPreview.tsx:PcbMesh.
 */

import { useMemo } from "react";
import * as THREE from "three";
import type { Pcb } from "@vcad/ir";

interface Props {
  pcb: Pcb;
}

export function PcbBoardMesh({ pcb }: Props) {
  const geometry = useMemo(() => {
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

    const thickness = pcb.outline.thickness;
    return new THREE.ExtrudeGeometry(shape, {
      depth: thickness,
      bevelEnabled: false,
    });
  }, [pcb.outline]);

  if (!geometry) return null;

  // Board is extruded in kernel Z-up space.
  // Center the extrusion vertically so the board surface is at Z = thickness/2.
  return (
    <mesh geometry={geometry} position={[0, 0, -pcb.outline.thickness / 2]}>
      <meshStandardMaterial
        color="#0d5a2d"
        roughness={0.8}
        metalness={0}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}
