/**
 * 3D ratsnest lines for unrouted connections.
 *
 * Dashed lines between same-net pads. Clickable to start routing.
 */

import { useMemo } from "react";
import type { Pcb } from "@vcad/ir";
import { computeRatsnest } from "@/lib/pcb-ratsnest";
import { layerZ } from "./pcb-geometry";
import type { NetlistResult } from "@vcad/engine";

interface Props {
  pcb: Pcb;
  netlist: NetlistResult | null;
  activeNet: string | null;
  hoveredNet: string | null;
  boardThickness: number;
  explosion: number;
  onStartRoute: (fpRef: string, padNum: string, net: string) => void;
  onHoverNet: (net: string | null) => void;
}

const ACCENT_COLOR = "#3b82f6";
const RATSNEST_COLOR = "#555555";

export function PcbRatsnest3D({
  pcb,
  netlist,
  activeNet,
  hoveredNet,
  boardThickness,
  explosion,
  onStartRoute,
  onHoverNet,
}: Props) {
  const ratsnest = useMemo(
    () => computeRatsnest(pcb.footprints, netlist, pcb.traces),
    [pcb.footprints, netlist, pcb.traces],
  );

  const z = layerZ("FCu", boardThickness, explosion) + 0.1;

  if (ratsnest.length === 0) return null;

  return (
    <group>
      {ratsnest.map((r, i) => {
        const isActive = r.net === activeNet || r.net === hoveredNet;
        const color = isActive ? ACCENT_COLOR : RATSNEST_COLOR;
        const posArray = new Float32Array([
          r.from.x, r.from.y, z,
          r.to.x, r.to.y, z,
        ]);
        return (
          <line
            key={i}
            onClick={(e) => {
              e.stopPropagation();
              onStartRoute(r.fpRef, r.padNum, r.net);
            }}
            onPointerEnter={() => onHoverNet(r.net)}
            onPointerLeave={() => onHoverNet(null)}
          >
            <bufferGeometry>
              <bufferAttribute
                attach="attributes-position"
                args={[posArray, 3]}
              />
            </bufferGeometry>
            <lineDashedMaterial
              color={color}
              dashSize={0.5}
              gapSize={0.5}
              linewidth={1}
            />
          </line>
        );
      })}
    </group>
  );
}
