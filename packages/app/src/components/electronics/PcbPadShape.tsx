/**
 * Renders one PCB pad as SVG elements.
 * Handles all PadShape variants: Circle, Rect, Oval, RoundRect, Custom.
 */

import type { Pad, PcbLayer } from "@vcad/ir";
import type { LayerConfig } from "@/stores/electronics-store";

interface PcbPadShapeProps {
  pad: Pad;
  layers: LayerConfig[];
  highlight: boolean;
  accentColor: string;
  onClick?: () => void;
  onPointerEnter?: () => void;
  onPointerLeave?: () => void;
}

function getLayerColor(layers: LayerConfig[], padLayers: PcbLayer[]): string {
  for (const pl of padLayers) {
    const cfg = layers.find((l) => l.layer === pl && l.visible);
    if (cfg) return cfg.color;
  }
  return "#888";
}

export function PcbPadShape({
  pad,
  layers,
  highlight,
  accentColor,
  onClick,
  onPointerEnter,
  onPointerLeave,
}: PcbPadShapeProps) {
  const color = highlight ? accentColor : getLayerColor(layers, pad.layers);
  const { shape } = pad;

  const common = {
    fill: color,
    opacity: highlight ? 1 : 0.85,
    className: "cursor-pointer",
    onClick,
    onPointerEnter,
    onPointerLeave,
    style: highlight
      ? { filter: `drop-shadow(0 0 3px ${accentColor}66)` }
      : undefined,
  };

  let padElement: React.ReactNode;

  switch (shape.type) {
    case "Circle":
      padElement = <circle cx={0} cy={0} r={shape.diameter / 2} {...common} />;
      break;
    case "Rect":
      padElement = (
        <rect
          x={-shape.width / 2}
          y={-shape.height / 2}
          width={shape.width}
          height={shape.height}
          {...common}
        />
      );
      break;
    case "Oval":
      padElement = (
        <ellipse
          cx={0}
          cy={0}
          rx={shape.width / 2}
          ry={shape.height / 2}
          {...common}
        />
      );
      break;
    case "RoundRect": {
      const r = Math.min(shape.width, shape.height) * (shape.cornerRatio ?? 0.25) * 0.5;
      padElement = (
        <rect
          x={-shape.width / 2}
          y={-shape.height / 2}
          width={shape.width}
          height={shape.height}
          rx={r}
          ry={r}
          {...common}
        />
      );
      break;
    }
    case "Custom":
      padElement = (
        <polygon
          points={shape.vertices.map((v) => `${v.x},${v.y}`).join(" ")}
          {...common}
        />
      );
      break;
  }

  // Drill hole
  const drillHole = pad.drill ? (
    <circle
      cx={0}
      cy={0}
      r={pad.drill.diameter / 2}
      fill="#111"
      pointerEvents="none"
    />
  ) : null;

  return (
    <g transform={`translate(${pad.position.x}, ${pad.position.y})${pad.rotation ? ` rotate(${pad.rotation})` : ""}`}>
      {padElement}
      {drillHole}
    </g>
  );
}
