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
 * Visible only when raytracing is active and the WebGPU init succeeded.
 * Opens a popover with quality preset, edge-detection toggle, and a way
 * to drop back to standard rendering — mirrors View → Ray Tracing so the
 * user doesn't have to climb back up to the menubar.
 */
export function RaytraceChip({ className }: { className?: string }) {
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const raytraceQuality = useUiStore((s) => s.raytraceQuality);
  const raytraceEdgesEnabled = useUiStore((s) => s.raytraceEdgesEnabled);
  const toggleRenderMode = useUiStore((s) => s.toggleRenderMode);
  const setRaytraceQuality = useUiStore((s) => s.setRaytraceQuality);
  const setRaytraceEdgesEnabled = useUiStore((s) => s.setRaytraceEdgesEnabled);

  if (renderMode !== "raytrace" || !raytraceAvailable) return null;

  return (
    <Popover.Root>
      <Tooltip side="top" content="Ray-tracing — quality, edges, turn off">
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
              className="shrink-0 text-violet-400"
            />
            <span className="uppercase tracking-wide tabular-nums text-text-muted">
              {QUALITY_SHORT[raytraceQuality]}
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

          <button
            type="button"
            onClick={toggleRenderMode}
            className={cn(
              "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left",
              "transition-colors",
              "text-text-muted hover:bg-hover hover:text-text",
            )}
          >
            <span className="flex-1 uppercase tracking-wide text-[10px] text-text">
              Turn off
            </span>
            <span className="uppercase tracking-wide text-[10px] text-text-muted/60">
              standard render
            </span>
          </button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
