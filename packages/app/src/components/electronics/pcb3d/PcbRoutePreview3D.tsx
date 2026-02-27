/**
 * In-progress route trace + clearance corridor as semi-transparent geometry.
 */

import type { Pcb, Vec2 } from "@vcad/ir";
import { layerZ } from "./pcb-geometry";

interface Props {
  pcb: Pcb;
  routeStartPad: { fpRef: string; padNum: string; net: string } | null;
  routePreview: Vec2[];
  boardThickness: number;
  activeLayer: string;
  explosion: number;
}

const ACCENT_COLOR = "#3b82f6";

export function PcbRoutePreview3D({ pcb, routeStartPad, routePreview, boardThickness, activeLayer, explosion }: Props) {
  if (!routeStartPad || routePreview.length === 0) return null;

  // Find start pad position
  const fp = pcb.footprints.find((f) => f.ref === routeStartPad.fpRef);
  const pad = fp?.pads.find((p) => p.number === routeStartPad.padNum);
  if (!fp || !pad) return null;

  const startPos = {
    x: fp.position.x + pad.position.x,
    y: fp.position.y + pad.position.y,
  };
  const endPos = routePreview[routePreview.length - 1]!;
  const z = layerZ(activeLayer as any, boardThickness, explosion) + 0.02;
  const traceWidth = pcb.rules.defaultRules.traceWidth;
  const clearance = pcb.rules.defaultRules.clearance;

  // Direction
  const dx = endPos.x - startPos.x;
  const dy = endPos.y - startPos.y;
  const length = Math.sqrt(dx * dx + dy * dy);
  if (length < 0.01) return null;

  const angle = Math.atan2(dy, dx);
  const cx = (startPos.x + endPos.x) / 2;
  const cy = (startPos.y + endPos.y) / 2;

  return (
    <group>
      {/* Clearance corridor */}
      <mesh
        position={[cx, cy, z - 0.01]}
        rotation={[0, 0, angle]}
      >
        <boxGeometry args={[length, traceWidth + clearance * 2, 0.02]} />
        <meshBasicMaterial color={ACCENT_COLOR} transparent opacity={0.15} />
      </mesh>

      {/* Trace preview */}
      <mesh
        position={[cx, cy, z]}
        rotation={[0, 0, angle]}
      >
        <boxGeometry args={[length, traceWidth, 0.035]} />
        <meshBasicMaterial color={ACCENT_COLOR} transparent opacity={0.7} />
      </mesh>
    </group>
  );
}
