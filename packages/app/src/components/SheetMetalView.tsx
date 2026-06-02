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

import { useDocumentStore, useEngineStore, useUiStore } from "@vcad/core";
import type {
  SheetMetalCostResult,
  SheetMetalFlatPattern,
  SheetMetalModelSummary,
  SheetMetalRendered,
  SheetMetalViolation,
} from "@vcad/engine";
import { useMemo, useState } from "react";
import { downloadBlob } from "@/lib/download";
import {
  useShopProfileStore,
  type ShopProfileNumberField,
} from "@/stores/shop-profile-store";

export function SheetMetalView() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const scene = useEngineStore((s) => s.scene);
  const engine = useEngineStore((s) => s.engine);
  const document = useDocumentStore((s) => s.document);
  const profile = useShopProfileStore((s) => s.profile);

  const rendered = useMemo<SheetMetalRendered | null>(() => {
    if (!scene || selectedPartIds.size !== 1) return null;
    for (const part of scene.parts) {
      if (part.sheetMetal) {
        return part.sheetMetal as SheetMetalRendered;
      }
    }
    return null;
  }, [scene, selectedPartIds]);

  // Re-run manufacturability against the user's saved shop. Falls back to
  // the ambient generic-shop result (from the eval pipeline) if the kernel
  // query is unavailable — e.g. an older WASM build without the binding.
  const checked = useMemo(() => {
    if (!engine || !rendered) return null;
    try {
      return engine.checkSheetMetal(document, profile);
    } catch (e) {
      console.warn("[sheet-metal] shop-profile check failed:", e);
      return null;
    }
  }, [engine, document, profile, rendered]);

  // Cost estimate — pure query, recomputes on any model edit. Generic
  // rates for now; later tier wires this to a persisted rate sheet.
  const [costQty, setCostQty] = useState(1);
  const cost = useMemo(() => {
    if (!engine || !rendered) return null;
    try {
      return engine.costSheetMetal(document, undefined, costQty);
    } catch (e) {
      console.warn("[sheet-metal] cost estimate failed:", e);
      return null;
    }
  }, [engine, document, rendered, costQty]);

  if (!rendered) return null;
  const { model, flatPattern, dxf } = rendered;
  const violations = checked?.violations ?? rendered.violations;
  const shopName = checked?.shop.name ?? profile.name;

  function handleDownloadDxf() {
    const blob = new Blob([dxf], { type: "application/dxf" });
    downloadBlob(blob, "flat-pattern.dxf");
  }

  return (
    <div className="flex w-full flex-col gap-3 border-t border-border/40 bg-surface p-3 text-[11px]">
      <Header model={model} flat={flatPattern} />
      <BendList model={model} />
      <DfmInspector violations={violations} shopName={shopName} />
      <ShopProfileEditor />
      <CostBadge cost={cost} qty={costQty} setQty={setCostQty} />
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
        <span>Material</span>
        <span className="text-text">{model.material || "—"}</span>
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
          const target_deg = (bend.angle_rad * 180) / Math.PI;
          const comp_deg = (bend.compensated_angle_rad * 180) / Math.PI;
          const springback_deg = (bend.springback_rad * 180) / Math.PI;
          const showSpringback = Math.abs(springback_deg) > 0.05;
          const isHem = bend.k_factor_source?.includes(";hem:");
          const label = isHem
            ? bend.k_factor_source?.includes(";hem:open")
              ? "Hem open"
              : "Hem closed"
            : `${target_deg.toFixed(0)}°`;
          return (
            <div
              key={i}
              className="flex flex-col gap-0.5 rounded bg-hover/30 px-2 py-1"
            >
              <div className="flex items-center justify-between gap-2">
                <div className="flex min-w-0 items-center gap-2">
                  <span
                    title={bend.k_factor_source ?? "no provenance"}
                    className="inline-block h-2 w-2 shrink-0 rounded-full"
                    style={{ backgroundColor: color }}
                  />
                  <span className="text-text">
                    #{i} {bend.direction} {label}
                  </span>
                </div>
                <div className="flex shrink-0 items-center gap-2 text-text-muted">
                  <span>R {bend.radius.toFixed(2)}</span>
                  <span>K {bend.k_factor.toFixed(3)}</span>
                  <span>BA {bend.allowance_mm.toFixed(2)}</span>
                </div>
              </div>
              {showSpringback && (
                <div
                  className="pl-4 text-[10px] text-text-muted/80"
                  title="Brake angle to form so the part springs back to the design angle"
                >
                  Form to {comp_deg.toFixed(1)}° (+{springback_deg.toFixed(1)}°
                  springback)
                </div>
              )}
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

function DfmInspector({
  violations,
  shopName,
}: {
  violations: SheetMetalViolation[];
  shopName: string;
}) {
  const errors = violations.filter((v) => v.severity === "Error").length;
  const warnings = violations.length - errors;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between">
        <span className="text-text-muted" title={`Checked against: ${shopName}`}>
          Manufacturability{" "}
          <span className="text-text-muted/60">· {shopName}</span>
        </span>
        {violations.length === 0 ? (
          <span className="text-[10px] font-medium text-[#22c55e]">
            Shop-ready
          </span>
        ) : (
          <span className="text-[10px] text-text-muted">
            {errors} {errors === 1 ? "error" : "errors"} · {warnings}{" "}
            {warnings === 1 ? "warning" : "warnings"}
          </span>
        )}
      </div>
      {violations.length > 0 && (
        <div className="flex flex-col gap-1">
          {violations.map((v, i) => (
            <div
              key={i}
              className="flex items-start gap-2 rounded bg-hover/30 px-2 py-1"
              title={v.rule}
            >
              <span
                className="mt-[3px] inline-block h-2 w-2 shrink-0 rounded-full"
                style={{
                  backgroundColor:
                    v.severity === "Error" ? "#ef4444" : "#f59e0b",
                }}
              />
              <span className="min-w-0 text-text">{v.message}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

const SHOP_FIELDS: {
  key: ShopProfileNumberField;
  label: string;
  unit: string;
  step: number;
}[] = [
  { key: "max_bend_length_mm", label: "Max bend length", unit: "mm", step: 50 },
  {
    key: "min_bend_radius_ratio",
    label: "Min bend radius",
    unit: "×t",
    step: 0.1,
  },
  { key: "min_flange_height_mm", label: "Min flange height", unit: "mm", step: 0.5 },
  { key: "min_hole_to_bend_mm", label: "Min hole→bend", unit: "mm", step: 0.5 },
  {
    key: "min_distance_between_bends_mm",
    label: "Min bend→bend",
    unit: "mm",
    step: 0.5,
  },
];

function ShopProfileEditor() {
  const profile = useShopProfileStore((s) => s.profile);
  const setName = useShopProfileStore((s) => s.setName);
  const setField = useShopProfileStore((s) => s.setField);
  const resetToGeneric = useShopProfileStore((s) => s.resetToGeneric);
  const [open, setOpen] = useState(false);

  return (
    <div className="flex flex-col gap-1">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center justify-between text-left text-text-muted transition-colors hover:text-text"
      >
        <span>Shop profile</span>
        <span className="text-[10px] text-text-muted/60">
          {open ? "Hide" : "Edit"}
        </span>
      </button>
      {open && (
        <div className="flex flex-col gap-1 rounded bg-hover/20 p-2">
          <label className="flex items-center justify-between gap-2">
            <span className="text-text-muted">Name</span>
            <input
              type="text"
              value={profile.name}
              onChange={(e) => setName(e.target.value)}
              className="w-32 rounded bg-surface px-1 py-0.5 text-right text-text outline-none focus:ring-1 focus:ring-accent"
            />
          </label>
          {SHOP_FIELDS.map((f) => (
            <label
              key={f.key}
              className="flex items-center justify-between gap-2"
            >
              <span className="text-text-muted">{f.label}</span>
              <span className="flex items-center gap-1">
                <input
                  type="number"
                  step={f.step}
                  min={0}
                  value={profile[f.key]}
                  onChange={(e) => {
                    const v = Number.parseFloat(e.target.value);
                    if (Number.isFinite(v) && v >= 0) setField(f.key, v);
                  }}
                  className="w-20 rounded bg-surface px-1 py-0.5 text-right text-text outline-none focus:ring-1 focus:ring-accent"
                />
                <span className="w-6 text-text-muted/60">{f.unit}</span>
              </span>
            </label>
          ))}
          <button
            type="button"
            onClick={resetToGeneric}
            className="mt-1 self-start rounded bg-hover/40 px-2 py-0.5 text-[10px] text-text-muted transition-colors hover:bg-hover hover:text-text"
          >
            Reset to generic
          </button>
        </div>
      )}
    </div>
  );
}

function CostBadge({
  cost,
  qty,
  setQty,
}: {
  cost: SheetMetalCostResult | null;
  qty: number;
  setQty: (n: number) => void;
}) {
  const [open, setOpen] = useState(false);
  if (!cost) return null;
  const b = cost.breakdown;
  const fmt = (v: number) =>
    `${b.currency} ${v.toFixed(v < 1 ? 3 : 2)}`;
  return (
    <div className="flex flex-col gap-1">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center justify-between rounded bg-hover/30 px-2 py-1 text-left transition-colors hover:bg-hover"
        title="Click for breakdown"
      >
        <span className="text-text-muted">Cost</span>
        <span className="flex items-center gap-2">
          <span className="font-medium text-text">{fmt(b.total_each)}</span>
          <span className="text-[10px] text-text-muted">
            each · qty {b.quantity}
          </span>
        </span>
      </button>
      {open && (
        <div className="flex flex-col gap-1 rounded bg-hover/20 p-2">
          <label className="flex items-center justify-between gap-2">
            <span className="text-text-muted">Quantity</span>
            <input
              type="number"
              min={1}
              step={1}
              value={qty}
              onChange={(e) => {
                const v = Number.parseInt(e.target.value, 10);
                if (Number.isFinite(v) && v >= 1) setQty(v);
              }}
              className="w-20 rounded bg-surface px-1 py-0.5 text-right text-text outline-none focus:ring-1 focus:ring-accent"
            />
          </label>
          <div className="grid grid-cols-2 gap-x-3 gap-y-0.5 text-text-muted">
            <span>Material ({b.mass_kg_each.toFixed(3)} kg)</span>
            <span className="text-right text-text">{fmt(b.material_each)}</span>
            <span>Cut ({b.cut_length_m.toFixed(2)} m)</span>
            <span className="text-right text-text">{fmt(b.cut_each)}</span>
            {b.pierces > 0 && (
              <>
                <span>Pierce ({b.pierces})</span>
                <span className="text-right text-text">
                  {fmt(b.pierce_each)}
                </span>
              </>
            )}
            <span>Bend ({b.bends})</span>
            <span className="text-right text-text">{fmt(b.bend_each)}</span>
            <span>Setup (amortized)</span>
            <span className="text-right text-text">{fmt(b.setup_each)}</span>
            <span>Markup</span>
            <span className="text-right text-text">{fmt(b.markup_each)}</span>
            <span className="font-medium text-text">Total each</span>
            <span className="text-right font-medium text-text">
              {fmt(b.total_each)}
            </span>
            <span>Total run</span>
            <span className="text-right text-text">{fmt(b.total_run)}</span>
          </div>
        </div>
      )}
    </div>
  );
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
