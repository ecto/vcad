/**
 * SheetMetalView — contextual side panel for a selected sheet-metal part.
 *
 * All data on display comes from the Rust kernel via
 * `EvaluatedPart.sheetMetal` (a {@link SheetMetalRendered} bundle attached
 * by the engine after calling `evaluateSheetMetalChain`). No geometric or
 * unfold math runs here — this component is rendering only.
 *
 * Shows:
 * - Header: thickness, panel/bend count, flat-pattern bbox + area.
 * - Per-bend list with K-factor and a colored provenance dot
 *   (green=builtin, blue=shop, purple=measured, amber=manual).
 * - SVG flat pattern (red dashed creases for bend-up, blue dashed for
 *   bend-down — matches the DXF layer convention).
 */

import { useEngineStore, useUiStore } from "@vcad/core";
import type {
  SheetMetalFlatPattern,
  SheetMetalModelSummary,
  SheetMetalRendered,
} from "@vcad/engine";
import { useMemo } from "react";
import { downloadBlob } from "@/lib/download";

export function SheetMetalView() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const scene = useEngineStore((s) => s.scene);

  const rendered = useMemo<SheetMetalRendered | null>(() => {
    if (!scene || selectedPartIds.size !== 1) return null;
    for (const part of scene.parts) {
      if (part.sheetMetal) {
        return part.sheetMetal as SheetMetalRendered;
      }
    }
    return null;
  }, [scene, selectedPartIds]);

  if (!rendered) return null;
  const { model, flatPattern, dxf } = rendered;

  function handleDownloadDxf() {
    const blob = new Blob([dxf], { type: "application/dxf" });
    downloadBlob(blob, "flat-pattern.dxf");
  }

  return (
    <div className="flex w-full flex-col gap-3 border-t border-border/40 bg-surface p-3 text-[11px]">
      <Header model={model} flat={flatPattern} />
      <BendList model={model} />
      <FlatPatternSvg flat={flatPattern} />
      <button
        type="button"
        onClick={handleDownloadDxf}
        className="rounded bg-hover/40 px-2 py-1 text-text-muted transition-colors hover:bg-hover hover:text-text"
        title="Layered DXF — CUT / BEND_UP / BEND_DOWN, millimetres"
      >
        Download DXF
      </button>
    </div>
  );
}

function Header({
  model,
  flat,
}: {
  model: SheetMetalModelSummary;
  flat: SheetMetalFlatPattern;
}) {
  const w = flat.bbox[2] - flat.bbox[0];
  const h = flat.bbox[3] - flat.bbox[1];
  return (
    <div className="flex flex-col gap-1">
      <div className="font-medium text-text">Sheet metal</div>
      <div className="grid grid-cols-2 gap-x-3 gap-y-1 text-text-muted">
        <span>Thickness</span>
        <span className="text-text">{model.thickness.toFixed(2)} mm</span>
        <span>Panels</span>
        <span className="text-text">{model.panel_count}</span>
        <span>Bends</span>
        <span className="text-text">{model.bend_count}</span>
        <span>Flat bbox</span>
        <span className="text-text">
          {w.toFixed(1)} × {Math.abs(h).toFixed(1)} mm
        </span>
        <span>Flat area</span>
        <span className="text-text">{flat.area_mm2.toFixed(0)} mm²</span>
      </div>
    </div>
  );
}

function BendList({ model }: { model: SheetMetalModelSummary }) {
  if (model.bend_count === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      <div className="text-text-muted">Bends</div>
      <div className="flex flex-col gap-1">
        {model.bends.map((bend, i) => {
          const color = provenanceDot(bend.k_factor_source);
          return (
            <div
              key={i}
              className="flex items-center justify-between gap-2 rounded bg-hover/30 px-2 py-1"
            >
              <div className="flex min-w-0 items-center gap-2">
                <span
                  title={bend.k_factor_source ?? "no provenance"}
                  className="inline-block h-2 w-2 shrink-0 rounded-full"
                  style={{ backgroundColor: color }}
                />
                <span className="text-text">
                  #{i} {bend.direction}{" "}
                  {((bend.angle_rad * 180) / Math.PI).toFixed(0)}°
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-2 text-text-muted">
                <span>R {bend.radius.toFixed(2)}</span>
                <span>K {bend.k_factor.toFixed(3)}</span>
                <span>BA {bend.allowance_mm.toFixed(2)}</span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function provenanceDot(source: string | null): string {
  if (!source) return "#888888";
  if (source.startsWith("builtin")) return "#22c55e"; // green
  if (source.startsWith("shop")) return "#3b82f6"; // blue
  if (source.startsWith("measured")) return "#a855f7"; // purple
  if (source === "manual") return "#f59e0b"; // amber
  return "#888888";
}

function FlatPatternSvg({ flat }: { flat: SheetMetalFlatPattern }) {
  const [minX, minY, maxX, maxY] = flat.bbox;
  const w = Math.max(1, maxX - minX);
  const h = Math.max(1, maxY - minY);
  const pad = Math.max(w, h) * 0.05;
  const viewBox = `${minX - pad} ${minY - pad} ${w + 2 * pad} ${h + 2 * pad}`;
  const stroke = Math.max(w, h) * 0.005;

  return (
    <div className="flex flex-col gap-1">
      <div className="text-text-muted">Flat pattern</div>
      <div className="rounded bg-black/20 p-2">
        <svg
          viewBox={viewBox}
          // Y is inverted in SVG vs CAD convention — flip it.
          style={{ width: "100%", height: "auto", transform: "scaleY(-1)" }}
        >
          {flat.panel_outlines_2d.map((outline, i) => (
            <polygon
              key={`o${i}`}
              points={outline.map(([x, y]) => `${x},${y}`).join(" ")}
              fill="rgba(255,255,255,0.04)"
              stroke="#ef4444"
              strokeWidth={stroke}
              strokeLinejoin="round"
            />
          ))}
          {flat.panel_holes_2d.flatMap((set, i) =>
            set.map((hole, j) => (
              <polygon
                key={`h${i}-${j}`}
                points={hole.map(([x, y]) => `${x},${y}`).join(" ")}
                fill="rgba(0,0,0,0.4)"
                stroke="#ef4444"
                strokeWidth={stroke}
              />
            )),
          )}
          {flat.creases.map((c, i) => {
            const color = c.direction === "Up" ? "#ef4444" : "#3b82f6";
            return (
              <line
                key={`c${i}`}
                x1={c.line[0][0]}
                y1={c.line[0][1]}
                x2={c.line[1][0]}
                y2={c.line[1][1]}
                stroke={color}
                strokeWidth={stroke}
                strokeDasharray={`${stroke * 4} ${stroke * 2}`}
              />
            );
          })}
        </svg>
        <div className="mt-1 flex items-center gap-3 text-[10px] text-text-muted">
          <Legend color="#ef4444" label="Cut" />
          <Legend color="#ef4444" label="Bend up" dashed />
          <Legend color="#3b82f6" label="Bend down" dashed />
        </div>
      </div>
    </div>
  );
}

function Legend({
  color,
  label,
  dashed,
}: {
  color: string;
  label: string;
  dashed?: boolean;
}) {
  return (
    <span className="flex items-center gap-1">
      <span
        className="inline-block h-[2px] w-4"
        style={{
          backgroundColor: dashed ? "transparent" : color,
          borderTop: dashed ? `2px dashed ${color}` : undefined,
        }}
      />
      {label}
    </span>
  );
}
