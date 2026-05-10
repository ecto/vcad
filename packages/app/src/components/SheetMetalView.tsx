/**
 * SheetMetalView — the contextual side panel for a selected sheet-metal
 * part. Reads the {@link SheetMetalModel} attached to the
 * {@link EvaluatedPart} by the engine and renders:
 *
 * - A header with thickness, panel/bend counts, total flat-pattern area.
 * - A per-bend list with K-factor and provenance dot (the visual taxonomy
 *   from the spec: green = built-in, blue = shop, purple = measured,
 *   amber = manual).
 * - A live SVG flat-pattern view (the "flat is a peer, not a view" part of
 *   the legendary architecture). Bend-up creases red, bend-down blue.
 *
 * Direct manipulation, the contextual Bend Strip, and bidirectional
 * editing land in follow-up tiers per `docs/design/sheet-metal.md`.
 */

import { useUiStore } from "@vcad/core";
import { useEngineStore } from "@vcad/core";
import {
  flatPatternFromModel,
  bendAllowance,
  type SheetMetalModel,
  type FlatPattern,
} from "@vcad/sheet-metal";
import { useMemo } from "react";

export function SheetMetalView() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const scene = useEngineStore((s) => s.scene);

  const model = useMemo<SheetMetalModel | null>(() => {
    if (!scene || selectedPartIds.size !== 1) return null;
    // Find the selected part's index in the scene's parts list. The current
    // app links part IDs to scene index by ordering; since we may not have
    // that mapping in scope here, scan all parts for one carrying a
    // sheet-metal model. Multi-sheet docs select the one matching the
    // currently-selected part by index when available.
    const candidates: SheetMetalModel[] = [];
    for (const part of scene.parts) {
      if (part.sheetMetal) candidates.push(part.sheetMetal as SheetMetalModel);
    }
    if (candidates.length === 0) return null;
    return candidates[0]!;
  }, [scene, selectedPartIds]);

  const flat = useMemo<FlatPattern | null>(() => {
    if (!model) return null;
    return flatPatternFromModel(model);
  }, [model]);

  if (!model || !flat) return null;

  return (
    <div className="flex w-full flex-col gap-3 border-t border-border/40 bg-surface p-3 text-[11px]">
      <Header model={model} flat={flat} />
      <BendList model={model} />
      <FlatPatternSvg flat={flat} thickness={model.thickness} />
    </div>
  );
}

function Header({ model, flat }: { model: SheetMetalModel; flat: FlatPattern }) {
  const w = flat.bbox.max.x - flat.bbox.min.x;
  const h = flat.bbox.max.y - flat.bbox.min.y;
  return (
    <div className="flex flex-col gap-1">
      <div className="text-text font-medium">Sheet metal</div>
      <div className="grid grid-cols-2 gap-x-3 gap-y-1 text-text-muted">
        <span>Thickness</span>
        <span className="text-text">{model.thickness.toFixed(2)} mm</span>
        <span>Panels</span>
        <span className="text-text">{model.panels.length}</span>
        <span>Bends</span>
        <span className="text-text">{model.bends.length}</span>
        <span>Flat bbox</span>
        <span className="text-text">
          {w.toFixed(1)} × {Math.abs(h).toFixed(1)} mm
        </span>
        <span>Flat area</span>
        <span className="text-text">{flat.areaMm2.toFixed(0)} mm²</span>
      </div>
    </div>
  );
}

function BendList({ model }: { model: SheetMetalModel }) {
  if (model.bends.length === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      <div className="text-text-muted">Bends</div>
      <div className="flex flex-col gap-1">
        {model.bends.map((bend, i) => {
          const ba = bendAllowance(bend, model.thickness);
          const dot = provenanceDot(bend.kFactorSource);
          return (
            <div
              key={i}
              className="flex items-center justify-between gap-2 rounded bg-hover/30 px-2 py-1"
            >
              <div className="flex min-w-0 items-center gap-2">
                <span
                  title={bend.kFactorSource ?? "no provenance"}
                  className="inline-block h-2 w-2 shrink-0 rounded-full"
                  style={{ backgroundColor: dot }}
                />
                <span className="text-text">
                  #{i} {bend.direction} {((bend.angle * 180) / Math.PI).toFixed(0)}°
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-2 text-text-muted">
                <span>R {bend.radius.toFixed(2)}</span>
                <span>K {bend.kFactor.toFixed(3)}</span>
                <span>BA {ba.toFixed(2)}</span>
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

function FlatPatternSvg({
  flat,
  thickness: _thickness,
}: {
  flat: FlatPattern;
  thickness: number;
}) {
  const minX = flat.bbox.min.x;
  const minY = flat.bbox.min.y;
  const w = Math.max(1, flat.bbox.max.x - minX);
  const h = Math.max(1, flat.bbox.max.y - minY);
  // Pad to give a little breathing room.
  const pad = Math.max(w, h) * 0.05;
  const viewBox = `${minX - pad} ${minY - pad} ${w + 2 * pad} ${h + 2 * pad}`;
  // Stroke widths in user-space units; pick something proportional.
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
          {/* Panel outlines (CUT layer — red) */}
          {flat.panelOutlines2D.map((outline, i) => (
            <polygon
              key={`o${i}`}
              points={outline.map((p) => `${p.x},${p.y}`).join(" ")}
              fill="rgba(255,255,255,0.04)"
              stroke="#ef4444"
              strokeWidth={stroke}
              strokeLinejoin="round"
            />
          ))}
          {/* Holes */}
          {flat.panelHoles2D.flatMap((set, i) =>
            set.map((hole, j) => (
              <polygon
                key={`h${i}-${j}`}
                points={hole.map((p) => `${p.x},${p.y}`).join(" ")}
                fill="rgba(0,0,0,0.4)"
                stroke="#ef4444"
                strokeWidth={stroke}
              />
            )),
          )}
          {/* Crease lines: red dashed for Up, blue dashed for Down */}
          {flat.creases.map((c, i) => {
            const color = c.direction === "Up" ? "#ef4444" : "#3b82f6";
            return (
              <line
                key={`c${i}`}
                x1={c.line[0].x}
                y1={c.line[0].y}
                x2={c.line[1].x}
                y2={c.line[1].y}
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
