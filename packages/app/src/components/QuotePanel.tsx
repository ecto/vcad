import { useState, useEffect, useRef } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { Clock } from "@phosphor-icons/react/dist/ssr/Clock";
import { Truck } from "@phosphor-icons/react/dist/ssr/Truck";
import { EnvelopeSimple } from "@phosphor-icons/react/dist/ssr/EnvelopeSimple";
import { cn } from "@/lib/utils";
import { useNotificationStore } from "@/stores/notification-store";
import {
  useOutputStore,
  MATERIAL_MAPPINGS,
  type MaterialType,
} from "@/stores/output-store";
import { useEngineStore, useDocumentStore } from "@vcad/core";
import { estimateCost } from "@vcad/engine";

interface MaterialOption {
  id: MaterialType;
  name: string;
  method: string;
  days: number;
}

const MATERIALS: MaterialOption[] = (
  Object.keys(MATERIAL_MAPPINGS) as MaterialType[]
).map((id) => ({
  id,
  ...MATERIAL_MAPPINGS[id].display,
}));

/**
 * Compute exact part volume (mm³) from the evaluated scene's meshes
 * using the divergence-theorem integral over each triangle. This is
 * the same formula `inspect_cad` uses on the MCP side, so the
 * QuotePanel total now agrees with `inspect_cad` byte-for-byte.
 */
function exactVolumeMm3(scene: { parts: { mesh: { positions: Float32Array; indices: Uint32Array } }[] } | null): number {
  if (!scene?.parts?.length) return 0;
  let total = 0;
  for (const part of scene.parts) {
    const { positions, indices } = part.mesh;
    let v = 0;
    for (let t = 0; t < indices.length; t += 3) {
      const i0 = indices[t]! * 3;
      const i1 = indices[t + 1]! * 3;
      const i2 = indices[t + 2]! * 3;
      const x1 = positions[i0]!,
        y1 = positions[i0 + 1]!,
        z1 = positions[i0 + 2]!;
      const x2 = positions[i1]!,
        y2 = positions[i1 + 1]!,
        z2 = positions[i1 + 2]!;
      const x3 = positions[i2]!,
        y3 = positions[i2 + 1]!,
        z3 = positions[i2 + 2]!;
      v +=
        (x1 * (y2 * z3 - y3 * z2) -
          x2 * (y1 * z3 - y3 * z1) +
          x3 * (y1 * z2 - y2 * z1)) /
        6.0;
    }
    total += Math.abs(v);
  }
  return total;
}

function AnimatedPrice({ value, duration = 500 }: { value: number; duration?: number }) {
  const [displayValue, setDisplayValue] = useState(0);
  const startTime = useRef<number | null>(null);
  const startValue = useRef(0);

  useEffect(() => {
    startValue.current = displayValue;
    startTime.current = null;

    function animate(timestamp: number) {
      if (startTime.current === null) {
        startTime.current = timestamp;
      }

      const elapsed = timestamp - startTime.current;
      const progress = Math.min(elapsed / duration, 1);

      // Ease out cubic
      const eased = 1 - Math.pow(1 - progress, 3);
      const current = startValue.current + (value - startValue.current) * eased;

      setDisplayValue(current);

      if (progress < 1) {
        requestAnimationFrame(animate);
      }
    }

    requestAnimationFrame(animate);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- intentionally exclude displayValue so animation starts from current value without restarting on every tick
  }, [value, duration]);

  return <>${displayValue.toFixed(2)}</>;
}

export function QuotePanel() {
  const [email, setEmail] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);

  const quotePanelOpen = useOutputStore((s) => s.quotePanelOpen);
  const closeQuotePanel = useOutputStore((s) => s.closeQuotePanel);
  const selectedMaterial = useOutputStore((s) => s.selectedMaterial);
  const setSelectedMaterial = useOutputStore((s) => s.setSelectedMaterial);

  const scene = useEngineStore((s) => s.scene);
  const parts = useDocumentStore((s) => s.parts);

  // Exact volume from the divergence-theorem integral; same formula
  // inspect_cad uses, so the panel and the MCP inspector agree.
  const volumeMm3 = exactVolumeMm3(scene);
  const volumeCm3 = volumeMm3 / 1000;

  // Async price + per-material previews come from the shared
  // estimateCost wrapper around vcad-kernel-cost. Results are cached
  // in useOutputStore so tooltips and toolbars elsewhere can read
  // the same number without re-fetching.
  const cachedPrices = useOutputStore((s) => s.cachedPrices);
  const setCachedPrices = useOutputStore((s) => s.setCachedPrices);
  const [weightG, setWeightG] = useState(0);

  useEffect(() => {
    let cancelled = false;
    if (volumeMm3 <= 0) {
      setCachedPrices({});
      setWeightG(0);
      return;
    }
    (async () => {
      const next: Partial<Record<MaterialType, number>> = {};
      for (const m of MATERIALS) {
        const mapping = MATERIAL_MAPPINGS[m.id];
        const est = await estimateCost({
          process: mapping.process,
          material: mapping.catalogName,
          partVolumeMm3: volumeMm3,
        });
        if (cancelled) return;
        next[m.id] = est.total_usd;
        if (m.id === selectedMaterial) setWeightG(est.weight_grams);
      }
      if (!cancelled) setCachedPrices(next);
    })();
    return () => {
      cancelled = true;
    };
  }, [volumeMm3, selectedMaterial, setCachedPrices]);

  const pricesByMaterial = cachedPrices;
  const price = pricesByMaterial[selectedMaterial] ?? 0;
  const selectedMaterialInfo = MATERIALS.find((m) => m.id === selectedMaterial)!;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!email) return;

    setIsSubmitting(true);

    // Simulate API call
    await new Promise((r) => setTimeout(r, 800));

    useNotificationStore.getState().addToast("You're on the waitlist!", "success");
    setEmail("");
    setIsSubmitting(false);
    closeQuotePanel();
  }

  // Close on escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && quotePanelOpen) {
        closeQuotePanel();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [quotePanelOpen, closeQuotePanel]);

  // Close on click outside
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        closeQuotePanel();
      }
    }
    if (quotePanelOpen) {
      // Delay to avoid immediate close from the button click
      const timeout = setTimeout(() => {
        window.addEventListener("click", handleClick);
      }, 100);
      return () => {
        clearTimeout(timeout);
        window.removeEventListener("click", handleClick);
      };
    }
  }, [quotePanelOpen, closeQuotePanel]);

  if (!quotePanelOpen) return null;

  return (
    <>
      {/* Backdrop for mobile */}
      <div className="fixed inset-0 z-40 bg-black/20 sm:hidden" onClick={closeQuotePanel} />

      {/* Panel - side panel on desktop, bottom sheet on mobile */}
      <div
        ref={panelRef}
        className={cn(
          "fixed z-50 bg-surface border border-border shadow-2xl",
          "animate-in fade-in-0",
          // Desktop: side panel
          "sm:top-14 sm:right-3 sm:w-80 sm:slide-in-from-right-4",
          // Mobile: bottom sheet
          "bottom-0 left-0 right-0 sm:bottom-auto sm:left-auto",
          "max-h-[80vh] sm:max-h-none overflow-auto",
          "slide-in-from-bottom-4 sm:slide-in-from-bottom-0",
        )}
      >
        {/* Preview banner */}
        <div className="bg-brand/10 border-b border-brand/20 px-4 py-2 text-center">
          <span className="text-[10px] font-medium uppercase tracking-wider text-brand">
            Preview — Manufacturing launches Q2
          </span>
        </div>

        {/* Header */}
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <h3 className="text-sm font-bold">Get a Quote</h3>
          <button
            onClick={closeQuotePanel}
            className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          >
            <X size={14} />
          </button>
        </div>

        {/* Part info */}
        <div className="border-b border-border px-4 py-3">
          <div className="flex items-center gap-2 text-xs text-text-muted">
            <Package size={14} />
            <span>
              {parts.length} part{parts.length !== 1 && "s"} · {volumeCm3.toFixed(1)} cm³ · ~{weightG.toFixed(0)}g
            </span>
          </div>
        </div>

        {/* Material selection */}
        <div className="p-4 space-y-2">
          <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted mb-3">
            Select Material
          </div>

          {MATERIALS.map((material) => {
            const materialPrice = pricesByMaterial[material.id] ?? 0;
            const isSelected = selectedMaterial === material.id;

            return (
              <button
                key={material.id}
                onClick={() => setSelectedMaterial(material.id)}
                className={cn(
                  "w-full flex items-center justify-between p-3 border transition-all",
                  isSelected
                    ? "border-brand bg-brand/5"
                    : "border-border hover:border-text-muted/30",
                )}
              >
                <div className="text-left">
                  <div className="text-sm font-medium">{material.name}</div>
                  <div className="flex items-center gap-2 text-[10px] text-text-muted">
                    <span>{material.method}</span>
                    <span>·</span>
                    <Clock size={10} />
                    <span>{material.days} days</span>
                  </div>
                </div>
                <div className={cn(
                  "text-sm font-bold",
                  isSelected && "text-brand",
                )}>
                  ${materialPrice.toFixed(2)}
                </div>
              </button>
            );
          })}
        </div>

        {/* Total */}
        <div className="border-t border-border px-4 py-3">
          <div className="flex items-center justify-between">
            <div>
              <div className="text-xs text-text-muted">Estimated Total</div>
              <div className="flex items-center gap-2 text-[10px] text-text-muted">
                <Truck size={10} />
                <span>Ships in {selectedMaterialInfo.days} days</span>
              </div>
            </div>
            <div className="text-xl font-bold text-brand">
              <AnimatedPrice value={price} />
            </div>
          </div>
        </div>

        {/* Waitlist form */}
        <form onSubmit={handleSubmit} className="border-t border-border p-4">
          <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted mb-3">
            Join the Waitlist
          </div>
          <div className="flex gap-2">
            <div className="relative flex-1">
              <EnvelopeSimple
                size={14}
                className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted"
              />
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="you@email.com"
                className={cn(
                  "w-full h-10 pl-9 pr-3 text-sm",
                  "bg-bg border border-border",
                  "placeholder:text-text-muted/50",
                  "focus:outline-none focus:border-brand",
                )}
              />
            </div>
            <button
              type="submit"
              disabled={!email || isSubmitting}
              className={cn(
                "h-10 px-4 text-xs font-bold uppercase tracking-wider",
                "bg-brand text-white",
                "hover:bg-[#d91e63]",
                "disabled:opacity-40 disabled:cursor-not-allowed",
                "transition-all",
              )}
            >
              {isSubmitting ? "..." : "Join"}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
