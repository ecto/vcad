/**
 * Per-footprint silkscreen/courtyard graphics + ref designator text.
 *
 * Renders Line2 segments for footprint graphics and drei <Text> for ref labels.
 */

import { useMemo } from "react";
import { Text } from "@react-three/drei";
import * as THREE from "three";
import type { Footprint, FootprintGraphic, PcbLayer } from "@vcad/ir";
import { layerZ, getLayerColor, isLayerVisible } from "./pcb-geometry";
import type { LayerConfig } from "@/stores/electronics-store";

interface Props {
  footprint: Footprint;
  layers: LayerConfig[];
  boardThickness: number;
  highlight: boolean;
  explosion: number;
}

function graphicToLines(
  graphic: FootprintGraphic,
  fpX: number,
  fpY: number,
  fpRot: number,
  boardThickness: number,
  explosion: number,
): { points: THREE.Vector3[]; color: string; layer: PcbLayer } | null {
  const z = layerZ(graphic.layer, boardThickness, explosion);
  const cos = Math.cos(fpRot);
  const sin = Math.sin(fpRot);

  const transform = (x: number, y: number): THREE.Vector3 => {
    const rx = x * cos - y * sin + fpX;
    const ry = x * sin + y * cos + fpY;
    return new THREE.Vector3(rx, ry, z);
  };

  switch (graphic.type) {
    case "Line":
      return {
        points: [transform(graphic.start.x, graphic.start.y), transform(graphic.end.x, graphic.end.y)],
        color: "#FFEB3B",
        layer: graphic.layer,
      };
    case "Rect": {
      const p1 = transform(graphic.start.x, graphic.start.y);
      const p2 = transform(graphic.end.x, graphic.start.y);
      const p3 = transform(graphic.end.x, graphic.end.y);
      const p4 = transform(graphic.start.x, graphic.end.y);
      return { points: [p1, p2, p3, p4, p1], color: "#FFEB3B", layer: graphic.layer };
    }
    case "Circle": {
      const segments = 24;
      const points: THREE.Vector3[] = [];
      for (let i = 0; i <= segments; i++) {
        const angle = (i / segments) * Math.PI * 2;
        const x = graphic.center.x + Math.cos(angle) * graphic.radius;
        const y = graphic.center.y + Math.sin(angle) * graphic.radius;
        points.push(transform(x, y));
      }
      return { points, color: "#FFEB3B", layer: graphic.layer };
    }
    case "Polygon": {
      const points = graphic.vertices.map((v) => transform(v.x, v.y));
      if (points.length > 0) points.push(points[0]!); // close
      return { points, color: "#FFEB3B", layer: graphic.layer };
    }
    default:
      return null;
  }
}

export function PcbFootprint3D({ footprint, layers, boardThickness, highlight, explosion }: Props) {
  const fpRot = (footprint.rotation ?? 0) * Math.PI / 180;

  const lineSegments = useMemo(() => {
    if (!footprint.graphics) return [];
    return footprint.graphics
      .map((g) => graphicToLines(g, footprint.position.x, footprint.position.y, fpRot, boardThickness, explosion))
      .filter((l): l is NonNullable<typeof l> => l !== null && isLayerVisible(layers, l.layer));
  }, [footprint, layers, boardThickness, fpRot, explosion]);

  const refZ = layerZ(footprint.front !== false ? "FSilkS" : "BSilkS", boardThickness, explosion);

  return (
    <group>
      {/* Silkscreen/courtyard graphics as line segments */}
      {lineSegments.map((seg, i) => (
        <line key={i}>
          <bufferGeometry>
            <bufferAttribute
              attach="attributes-position"
              args={[new Float32Array(seg.points.flatMap((p) => [p.x, p.y, p.z])), 3]}
            />
          </bufferGeometry>
          <lineBasicMaterial
            color={highlight ? "#3b82f6" : getLayerColor(layers, seg.layer)}
            linewidth={1}
          />
        </line>
      ))}

      {/* Ref designator text */}
      <Text
        position={[footprint.position.x, footprint.position.y, refZ + 0.05]}
        rotation={[0, 0, fpRot]}
        fontSize={0.8}
        color={highlight ? "#3b82f6" : "#FFEB3B"}
        anchorX="center"
        anchorY="middle"
        outlineWidth={0.05}
        outlineColor="#000000"
      >
        {footprint.ref}
      </Text>
    </group>
  );
}
