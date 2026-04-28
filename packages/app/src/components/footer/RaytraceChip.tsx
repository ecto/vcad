import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import * as Popover from "@radix-ui/react-popover";
import { useUiStore, type RaytraceQuality } from "@vcad/core";
import { FooterChipButton } from "@/components/footer/FooterChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const QUALITY_OPTIONS: Array<{
  value: RaytraceQuality;
  label: string;
  hint: string;
}> = [
  { value: "draft", label: "Draft", hint: "Fastest — half resolution" },
  { value: "standard", label: "Standard", hint: "720p cap" },
  { value: "high", label: "High", hint: "1080p cap, 2× DPI" },
];

const QUALITY_SHORT: Record<RaytraceQuality, string> = {
  draft: "DRAFT",
  standard: "STD",
  high: "HIGH",
};

/**
 * Render-mode pill for the footer.
 *
 * Always visible when WebGPU ray tracing is supported (regardless of
 * whether raytrace mode is currently active) — acts as a quick toggle
 * plus the controls popover. Hidden entirely if WebGPU init failed.
 *
 * When raytrace is OFF the icon dims and the readout reads "OFF"; when
 * ON the icon glows violet and the readout shows the active tier.
 */
export function RaytraceChip({ className }: { className?: string }) {
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const raytraceQuality = useUiStore((s) => s.raytraceQuality);
  const raytraceEdgesEnabled = useUiStore((s) => s.raytraceEdgesEnabled);
  const raytraceEdgeSilhouetteEnabled = useUiStore((s) => s.raytraceEdgeSilhouetteEnabled);
  const raytraceEdgeCreaseEnabled = useUiStore((s) => s.raytraceEdgeCreaseEnabled);
  const raytraceEdgeBoundaryEnabled = useUiStore((s) => s.raytraceEdgeBoundaryEnabled);
  const raytraceEdgeSilhouetteWidth = useUiStore((s) => s.raytraceEdgeSilhouetteWidth);
  const raytraceEdgeCreaseWidth = useUiStore((s) => s.raytraceEdgeCreaseWidth);
  const raytraceEdgeBoundaryWidth = useUiStore((s) => s.raytraceEdgeBoundaryWidth);
  const raytraceEdgeSoftness = useUiStore((s) => s.raytraceEdgeSoftness);
  const toggleRenderMode = useUiStore((s) => s.toggleRenderMode);
  const setRaytraceQuality = useUiStore((s) => s.setRaytraceQuality);
  const setRaytraceEdgesEnabled = useUiStore((s) => s.setRaytraceEdgesEnabled);
  const setRaytraceEdgeSilhouetteEnabled = useUiStore((s) => s.setRaytraceEdgeSilhouetteEnabled);
  const setRaytraceEdgeCreaseEnabled = useUiStore((s) => s.setRaytraceEdgeCreaseEnabled);
  const setRaytraceEdgeBoundaryEnabled = useUiStore((s) => s.setRaytraceEdgeBoundaryEnabled);
  const setRaytraceEdgeSilhouetteWidth = useUiStore((s) => s.setRaytraceEdgeSilhouetteWidth);
  const setRaytraceEdgeCreaseWidth = useUiStore((s) => s.setRaytraceEdgeCreaseWidth);
  const setRaytraceEdgeBoundaryWidth = useUiStore((s) => s.setRaytraceEdgeBoundaryWidth);
  const setRaytraceEdgeSoftness = useUiStore((s) => s.setRaytraceEdgeSoftness);

  if (!raytraceAvailable) return null;

  const isOn = renderMode === "raytrace";

  return (
    <Popover.Root>
      <Tooltip
        side="top"
        content={isOn ? "Ray-tracing — quality, edges, turn off" : "Ray-tracing — turn on, quality, edges"}
      >
        <Popover.Trigger asChild>
          <FooterChipButton
            className={cn(
              "gap-1.5 px-2",
              "animate-in fade-in duration-300",
              className,
            )}
          >
            <Sparkle
              size={11}
              weight="fill"
              className={cn(
                "shrink-0 transition-colors",
                isOn ? "text-violet-400" : "text-text-muted/50",
              )}
            />
            <span
              className={cn(
                "uppercase tracking-wide tabular-nums transition-colors",
                isOn ? "text-text-muted" : "text-text-muted/60",
              )}
            >
              {isOn ? QUALITY_SHORT[raytraceQuality] : "OFF"}
            </span>
          </FooterChipButton>
        </Popover.Trigger>
      </Tooltip>
      <Popover.Portal>
        <Popover.Content
          side="top"
          align="end"
          sideOffset={6}
          collisionPadding={8}
          className={cn(
            "z-50 w-[260px]",
            "rounded-md border border-border/60 bg-surface/95 backdrop-blur-md",
            "p-1 shadow-xl",
            "animate-in fade-in slide-in-from-bottom-2 duration-150",
            "text-[11px]",
          )}
        >
          <div className="px-2 pt-1 pb-1.5 text-text-muted/60 uppercase tracking-[0.15em] text-[9px]">
            Quality
          </div>
          {QUALITY_OPTIONS.map(({ value, label, hint }) => {
            const active = raytraceQuality === value;
            return (
              <button
                key={value}
                type="button"
                onClick={() => setRaytraceQuality(value)}
                className={cn(
                  "flex w-full items-start gap-2 rounded-sm px-2 py-1.5 text-left",
                  "transition-colors",
                  active
                    ? "bg-brand/15 text-text"
                    : "text-text-muted hover:bg-hover hover:text-text",
                )}
              >
                <span className="flex-1">
                  <span
                    className={cn(
                      "block uppercase tracking-wide text-[10px]",
                      active ? "text-brand" : "text-text",
                    )}
                  >
                    {label}
                  </span>
                  <span className="block text-[10px] text-text-muted/70">
                    {hint}
                  </span>
                </span>
                {active && (
                  <Check
                    size={11}
                    weight="bold"
                    className="mt-0.5 shrink-0 text-brand"
                  />
                )}
              </button>
            );
          })}

          <div className="my-1 border-t border-border/40" />

          {/* Master edges toggle */}
          <button
            type="button"
            onClick={() => setRaytraceEdgesEnabled(!raytraceEdgesEnabled)}
            className={cn(
              "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left",
              "transition-colors",
              "text-text-muted hover:bg-hover hover:text-text",
            )}
          >
            <span className="flex-1 uppercase tracking-wide text-[10px] text-text">
              Edges
            </span>
            <span
              className={cn(
                "uppercase tracking-wide text-[10px] tabular-nums",
                raytraceEdgesEnabled ? "text-brand" : "text-text-muted/70",
              )}
            >
              {raytraceEdgesEnabled ? "on" : "off"}
            </span>
          </button>

          {/* Per-type edge controls (visible when edges are on) */}
          {raytraceEdgesEnabled && (
            <div className="ml-2 mt-0.5 space-y-0.5">
              {([
                {
                  label: "Silhouette",
                  enabled: raytraceEdgeSilhouetteEnabled,
                  setEnabled: setRaytraceEdgeSilhouetteEnabled,
                  width: raytraceEdgeSilhouetteWidth,
                  setWidth: setRaytraceEdgeSilhouetteWidth,
                },
                {
                  label: "Crease",
                  enabled: raytraceEdgeCreaseEnabled,
                  setEnabled: setRaytraceEdgeCreaseEnabled,
                  width: raytraceEdgeCreaseWidth,
                  setWidth: setRaytraceEdgeCreaseWidth,
                },
                {
                  label: "Boundary",
                  enabled: raytraceEdgeBoundaryEnabled,
                  setEnabled: setRaytraceEdgeBoundaryEnabled,
                  width: raytraceEdgeBoundaryWidth,
                  setWidth: setRaytraceEdgeBoundaryWidth,
                },
              ] as const).map(({ label, enabled, setEnabled, width, setWidth }) => (
                <div key={label} className="flex items-center gap-2 px-2 py-0.5">
                  <button
                    type="button"
                    onClick={() => setEnabled(!enabled)}
                    className={cn(
                      "w-14 text-left uppercase tracking-wide text-[9px]",
                      "transition-colors",
                      enabled ? "text-text" : "text-text-muted/50",
                    )}
                  >
                    {label}
                  </button>
                  <input
                    type="range"
                    min={0.25}
                    max={3}
                    step={0.25}
                    value={width}
                    disabled={!enabled}
                    onChange={(e) => setWidth(Number(e.target.value))}
                    className="flex-1 h-1 accent-brand disabled:opacity-40"
                  />
                  <span className="w-6 text-right text-[9px] tabular-nums text-text-muted/60">
                    {width.toFixed(2)}
                  </span>
                </div>
              ))}
              {/* Softness (AA falloff) */}
              <div className="flex items-center gap-2 px-2 py-0.5">
                <span className="w-14 uppercase tracking-wide text-[9px] text-text-muted/70">
                  Softness
                </span>
                <input
                  type="range"
                  min={0.5}
                  max={4}
                  step={0.5}
                  value={raytraceEdgeSoftness}
                  onChange={(e) => setRaytraceEdgeSoftness(Number(e.target.value))}
                  className="flex-1 h-1 accent-brand"
                />
                <span className="w-6 text-right text-[9px] tabular-nums text-text-muted/60">
                  {raytraceEdgeSoftness.toFixed(1)}
                </span>
              </div>
            </div>
          )}

          <div className="my-1 border-t border-border/40" />

          <button
            type="button"
            onClick={toggleRenderMode}
            className={cn(
              "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left",
              "transition-colors",
              "text-text-muted hover:bg-hover hover:text-text",
            )}
          >
            <span
              className={cn(
                "flex-1 uppercase tracking-wide text-[10px]",
                isOn ? "text-text" : "text-brand",
              )}
            >
              {isOn ? "Turn off" : "Turn on"}
            </span>
            <span className="uppercase tracking-wide text-[10px] text-text-muted/60">
              {isOn ? "standard render" : "ray-trace this scene"}
            </span>
          </button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
