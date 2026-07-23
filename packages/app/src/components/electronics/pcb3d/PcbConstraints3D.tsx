/**
 * Design-constraint overlay for the 3D board view: glyphs at constraint
 * midpoints, dimension leaders with values (driven dimensions dashed and
 * muted — measurements, not drivers), and orange rings on the targets the
 * constrain tool has picked so far.
 */

import { useMemo } from "react";
import { Line, Html } from "@react-three/drei";
import type { DesignConstraint, Pcb, Anchor } from "@vcad/ir";
import type { ConstraintTarget } from "@/stores/electronics-store";
import { layerZ } from "./pcb-geometry";

interface Props {
  pcb: Pcb;
  nodeId: number;
  constraints: DesignConstraint[];
  pickedTargets: ConstraintTarget[];
  boardThickness: number;
  explosion: number;
}

const GLYPHS: Record<string, string> = {
  coincident: "⌖",
  concentric: "◎",
  horizontal: "—",
  vertical: "|",
  parallel: "∥",
  perpendicular: "⊥",
  equalLength: "=",
  fixed: "⚓",
  symmetric: "⋈",
  pointOnEdge: "⌐",
};

/** Board-frame position of a point-like anchor, or null. */
function anchorPos(pcb: Pcb, nodeId: number, a: Anchor): { x: number; y: number } | null {
  if (Number((a as { node?: number }).node) !== nodeId) return null;
  if (a.kind === "pcbFootprint") {
    const fp = pcb.footprints.find((f) => f.ref === a.ref);
    if (!fp) return null;
    if (a.pad) {
      const pad = fp.pads.find((p) => p.number === a.pad);
      if (!pad) return null;
      const ang = ((fp.rotation ?? 0) * Math.PI) / 180;
      return {
        x: fp.position.x + pad.position.x * Math.cos(ang) - pad.position.y * Math.sin(ang),
        y: fp.position.y + pad.position.x * Math.sin(ang) + pad.position.y * Math.cos(ang),
      };
    }
    return { x: fp.position.x, y: fp.position.y };
  }
  if (a.kind === "pcbOutlineVertex") {
    const v = pcb.outline.vertices[a.index];
    return v ? { x: v.x, y: v.y } : null;
  }
  if (a.kind === "pcbOutlineEdge") {
    const n = pcb.outline.vertices.length;
    const v1 = pcb.outline.vertices[a.index];
    const v2 = pcb.outline.vertices[(a.index + 1) % n];
    return v1 && v2 ? { x: (v1.x + v2.x) / 2, y: (v1.y + v2.y) / 2 } : null;
  }
  return null;
}

function constraintAnchors(kind: DesignConstraint["kind"]): Anchor[] {
  return Object.values(kind as unknown as Record<string, unknown>).filter(
    (v): v is Anchor => v != null && typeof v === "object" && "kind" in (v as object),
  );
}

function labelStyle(driven: boolean): React.CSSProperties {
  return {
    pointerEvents: "none",
    fontSize: "10px",
    fontFamily: "JetBrains Mono, monospace",
    whiteSpace: "nowrap",
    color: driven ? "var(--text-muted, #9ca3af)" : "var(--text, #e5e7eb)",
    fontStyle: driven ? "italic" : "normal",
    background: "rgba(0,0,0,0.55)",
    padding: "1px 4px",
    borderRadius: "3px",
  };
}

export function PcbConstraints3D({
  pcb,
  nodeId,
  constraints,
  pickedTargets,
  boardThickness,
  explosion,
}: Props) {
  const z = layerZ("FCu", boardThickness, explosion) + 0.12;

  const items = useMemo(() => {
    return constraints
      .map((c) => {
        const anchors = constraintAnchors(c.kind);
        const points = anchors
          .map((a) => anchorPos(pcb, nodeId, a))
          .filter((p): p is { x: number; y: number } => p !== null);
        if (points.length === 0) return null;
        const mid = {
          x: points.reduce((s, p) => s + p.x, 0) / points.length,
          y: points.reduce((s, p) => s + p.y, 0) / points.length,
        };
        const type = (c.kind as { type: string }).type;
        const value = (c.kind as { value?: number | string }).value;
        return { c, type, value, points, mid };
      })
      .filter((x): x is NonNullable<typeof x> => x !== null);
  }, [constraints, pcb, nodeId]);

  const picked = useMemo(
    () =>
      pickedTargets
        .map((t) =>
          t.kind === "footprint"
            ? pcb.footprints.find((f) => f.ref === t.ref)?.position
            : pcb.outline.vertices[t.idx],
        )
        .filter((p): p is { x: number; y: number } => p != null),
    [pickedTargets, pcb],
  );

  if (items.length === 0 && picked.length === 0) return null;

  return (
    <group>
      {items.map(({ c, type, value, points, mid }) => {
        const dimensional = value !== undefined;
        const driven = c.driven === true;
        return (
          <group key={c.id}>
            {dimensional && points.length === 2 && (
              <Line
                points={[
                  [points[0]!.x, points[0]!.y, z],
                  [points[1]!.x, points[1]!.y, z],
                ]}
                color={driven ? "#9ca3af" : "#e5e7eb"}
                lineWidth={1}
                dashed={driven}
                dashSize={0.5}
                gapSize={0.35}
                transparent
                opacity={0.75}
              />
            )}
            <Html position={[mid.x, mid.y, z]} center zIndexRange={[20, 0]}>
              <div style={labelStyle(driven)}>
                {dimensional
                  ? `${typeof value === "number" ? value.toFixed(2) : String(value)}${
                      type === "angle" || type === "rotation" ? "°" : ""
                    }${driven ? " (ref)" : ""}`
                  : (GLYPHS[type] ?? type)}
              </div>
            </Html>
          </group>
        );
      })}

      {/* Orange = action: targets picked by the constrain tool */}
      {picked.map((p, i) => (
        <mesh key={`pick-${i}`} position={[p.x, p.y, z + 0.02]}>
          <ringGeometry args={[1.0, 1.25, 24]} />
          <meshBasicMaterial color="#f97316" transparent opacity={0.9} />
        </mesh>
      ))}
    </group>
  );
}
