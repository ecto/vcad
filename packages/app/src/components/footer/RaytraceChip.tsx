import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import * as Popover from "@radix-ui/react-popover";
import { useUiStore, type RaytraceQuality } from "@vcad/core";
import { FooterChipButton } from "@/components/footer/FooterChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useCallback } from "react";

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
  const raytraceAoEnabled = useUiStore((s) => s.raytraceAoEnabled);
  const raytraceAoIntensity = useUiStore((s) => s.raytraceAoIntensity);
  const raytraceAoSampleCount = useUiStore((s) => s.raytraceAoSampleCount);
  const toggleRenderMode = useUiStore((s) => s.toggleRenderMode);
  const setRaytraceQuality = useUiStore((s) => s.setRaytraceQuality);
  const setRaytraceEdgesEnabled = useUiStore((s) => s.setRaytraceEdgesEnabled);
  const setRaytraceAoEnabled = useUiStore((s) => s.setRaytraceAoEnabled);
  const setRaytraceAoIntensity = useUiStore((s) => s.setRaytraceAoIntensity);
  const setRaytraceAoSampleCount = useUiStore((s) => s.setRaytraceAoSampleCount);

  const handleAoIntensityChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      setRaytraceAoIntensity(parseFloat(e.target.value));
    },
    [setRaytraceAoIntensity],
  );

  const handleAoSampleChange = useCallback(
    (count: number) => {
      setRaytraceAoSampleCount(count);
    },
    [setRaytraceAoSampleCount],
  );

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

          <div className="my-1 border-t border-border/40" />

          {/* AO toggle + intensity slider */}
          <button
            type="button"
            onClick={() => setRaytraceAoEnabled(!raytraceAoEnabled)}
            className={cn(
              "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left",
              "transition-colors",
              "text-text-muted hover:bg-hover hover:text-text",
            )}
          >
            <span className="flex-1 uppercase tracking-wide text-[10px] text-text">
              Ambient Occlusion
            </span>
            <span
              className={cn(
                "uppercase tracking-wide text-[10px] tabular-nums",
                raytraceAoEnabled ? "text-brand" : "text-text-muted/70",
              )}
            >
              {raytraceAoEnabled ? "on" : "off"}
            </span>
          </button>

          {raytraceAoEnabled && (
            <div className="px-2 pb-1.5 flex flex-col gap-1">
              <div className="flex items-center gap-2">
                <span className="text-[10px] text-text-muted/70 w-16 shrink-0">Intensity</span>
                <input
                  type="range"
                  min="0.1"
                  max="2.0"
                  step="0.05"
                  value={raytraceAoIntensity}
                  onChange={handleAoIntensityChange}
                  className="flex-1 h-1 accent-brand"
                />
                <span className="text-[10px] text-text-muted tabular-nums w-8 text-right">
                  {raytraceAoIntensity.toFixed(2)}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <span className="text-[10px] text-text-muted/70 w-16 shrink-0">Samples</span>
                <div className="flex gap-1">
                  {([8, 16, 32] as const).map((n) => (
                    <button
                      key={n}
                      type="button"
                      onClick={() => handleAoSampleChange(n)}
                      className={cn(
                        "px-1.5 py-0.5 rounded text-[10px] tabular-nums",
                        raytraceAoSampleCount === n
                          ? "bg-brand/20 text-brand"
                          : "text-text-muted hover:bg-hover",
                      )}
                    >
                      {n}
                    </button>
                  ))}
                </div>
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
