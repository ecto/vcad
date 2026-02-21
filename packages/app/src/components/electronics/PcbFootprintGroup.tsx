/**
 * Renders one PCB footprint: transform group, pads, silk graphics, ref text.
 */

import type { Footprint, PcbLayer } from "@vcad/ir";
import type { LayerConfig } from "@/stores/electronics-store";
import { PcbPadShape } from "./PcbPadShape";
import { useElectronicsStore } from "@/stores/electronics-store";

interface PcbFootprintGroupProps {
  footprint: Footprint;
  layers: LayerConfig[];
  highlight: boolean;
  accentColor: string;
}

function getLayerColor(layers: LayerConfig[], layer: PcbLayer): string {
  const cfg = layers.find((l) => l.layer === layer);
  return cfg?.color ?? "#888";
}

function isLayerVisible(layers: LayerConfig[], layer: PcbLayer): boolean {
  const cfg = layers.find((l) => l.layer === layer);
  return cfg?.visible ?? false;
}

export function PcbFootprintGroup({
  footprint,
  layers,
  highlight,
  accentColor,
}: PcbFootprintGroupProps) {
  const select = useElectronicsStore((s) => s.select);
  const setHoveredNet = useElectronicsStore((s) => s.setHoveredNet);
  const inferLayer = useElectronicsStore((s) => s.inferLayerFromPad);
  const startRoute = useElectronicsStore((s) => s.startRoute);
  const pcbTool = useElectronicsStore((s) => s.pcbTool);
  const selection = useElectronicsStore((s) => s.selection);
  const hoveredNet = useElectronicsStore((s) => s.hoveredNet);

  // Check if a net is active (selected or hovered)
  const activeNet =
    selection.type === "net"
      ? selection.netId
      : selection.type === "trace" || selection.type === "via" || selection.type === "pad"
        ? selection.net
        : null;

  const isNetHighlighted = (net: string | undefined) =>
    net !== undefined && (net === activeNet || net === hoveredNet);

  return (
    <g
      transform={`translate(${footprint.position.x}, ${footprint.position.y})${footprint.rotation ? ` rotate(${footprint.rotation})` : ""}`}
      className="cursor-pointer"
      onClick={(e) => {
        e.stopPropagation();
        select({ type: "footprint", ref: footprint.ref });
      }}
    >
      {/* Silkscreen / courtyard graphics */}
      {(footprint.graphics ?? []).map((g, i) => {
        if (!isLayerVisible(layers, g.layer)) return null;
        const color = getLayerColor(layers, g.layer);

        switch (g.type) {
          case "Line":
            return (
              <line
                key={i}
                x1={g.start.x}
                y1={g.start.y}
                x2={g.end.x}
                y2={g.end.y}
                stroke={color}
                strokeWidth={g.width || 0.15}
                fill="none"
              />
            );
          case "Circle":
            return (
              <circle
                key={i}
                cx={g.center.x}
                cy={g.center.y}
                r={g.radius}
                stroke={color}
                strokeWidth={g.width || 0.15}
                fill="none"
              />
            );
          case "Arc": {
            const startRad = (g.startAngle * Math.PI) / 180;
            const endRad = (g.endAngle * Math.PI) / 180;
            const x1 = g.center.x + g.radius * Math.cos(startRad);
            const y1 = g.center.y + g.radius * Math.sin(startRad);
            const x2 = g.center.x + g.radius * Math.cos(endRad);
            const y2 = g.center.y + g.radius * Math.sin(endRad);
            const largeArc = Math.abs(g.endAngle - g.startAngle) > 180 ? 1 : 0;
            return (
              <path
                key={i}
                d={`M ${x1} ${y1} A ${g.radius} ${g.radius} 0 ${largeArc} 1 ${x2} ${y2}`}
                stroke={color}
                strokeWidth={g.width || 0.15}
                fill="none"
              />
            );
          }
          case "Rect":
            return (
              <rect
                key={i}
                x={Math.min(g.start.x, g.end.x)}
                y={Math.min(g.start.y, g.end.y)}
                width={Math.abs(g.end.x - g.start.x)}
                height={Math.abs(g.end.y - g.start.y)}
                stroke={color}
                strokeWidth={g.width || 0.15}
                fill="none"
              />
            );
          case "Polygon":
            return (
              <polygon
                key={i}
                points={g.vertices.map((v) => `${v.x},${v.y}`).join(" ")}
                stroke={color}
                strokeWidth={g.width || 0.15}
                fill="none"
              />
            );
          case "Text":
            return (
              <text
                key={i}
                x={g.position.x}
                y={g.position.y}
                fontSize={g.height}
                fill={color}
                fontFamily="monospace"
                transform={g.rotation ? `rotate(${g.rotation}, ${g.position.x}, ${g.position.y})` : undefined}
              >
                {g.text}
              </text>
            );
          default:
            return null;
        }
      })}

      {/* Pads */}
      {footprint.pads.map((pad, i) => {
        const padHighlight =
          highlight || isNetHighlighted(pad.net);

        return (
          <PcbPadShape
            key={i}
            pad={pad}
            layers={layers}
            highlight={padHighlight}
            accentColor={accentColor}
            onClick={() => {
              // Principle 5: layer follows intent
              inferLayer(pad.layers);
              if (pcbTool === "route" && pad.net) {
                startRoute(footprint.ref, pad.number, pad.net);
              } else if (pad.net) {
                select({ type: "pad", fpRef: footprint.ref, padNum: pad.number, net: pad.net });
              }
            }}
            onPointerEnter={() => pad.net && setHoveredNet(pad.net)}
            onPointerLeave={() => setHoveredNet(null)}
          />
        );
      })}

      {/* Reference text */}
      <text
        x={0}
        y={-2}
        fontSize={1.2}
        fill={highlight ? accentColor : getLayerColor(layers, "FSilkS")}
        fontFamily="monospace"
        textAnchor="middle"
        pointerEvents="none"
      >
        {footprint.ref}
      </text>
    </g>
  );
}
