import { Wrench } from "@phosphor-icons/react/dist/ssr/Wrench";
import { useDfmStore, severityCounts } from "@/stores/dfm-store";
import { FooterChipButton } from "@/components/footer/FooterChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/**
 * Manufacturability chip — surfaces the live DFM check in the status bar
 * and acts as the entry point to the bottom drawer where issues are
 * triaged. Always visible (the drawer is dismissible, the chip is the
 * anchor) so users notice when a model picks up an error / warning.
 *
 * When Live check is disabled the chip dims to "OFF". Clicking still
 * opens the drawer so the user can flip it back on.
 */
export function DfmChip({ className }: { className?: string }) {
  const enabled = useDfmStore((s) => s.enabled);
  const running = useDfmStore((s) => s.running);
  const report = useDfmStore((s) => s.report);
  const drawerOpen = useDfmStore((s) => s.drawerOpen);
  const toggleDrawer = useDfmStore((s) => s.toggleDrawer);

  const counts = severityCounts(report);
  const total = counts.error + counts.warning + counts.info;

  const tone = !enabled
    ? "text-text-muted/50"
    : counts.error > 0
      ? "text-red-400"
      : counts.warning > 0
        ? "text-amber-400"
        : "text-emerald-400";

  return (
    <Tooltip
      side="top"
      content={
        !enabled
          ? "Manufacturability — disabled"
          : running
            ? "Manufacturability — checking…"
            : total === 0
              ? "Manufacturability — no issues"
              : `Manufacturability — ${counts.error} error, ${counts.warning} warning, ${counts.info} info`
      }
    >
      <FooterChipButton
        onClick={toggleDrawer}
        aria-pressed={drawerOpen}
        className={cn("gap-1.5 px-2", className)}
      >
        <Wrench
          size={11}
          weight="fill"
          className={cn("shrink-0 transition-colors", tone)}
        />
        {!enabled ? (
          <span className="uppercase tracking-wide tabular-nums text-text-muted/60">
            OFF
          </span>
        ) : total === 0 && !running ? (
          <span className="uppercase tracking-wide text-text-muted">OK</span>
        ) : (
          <span className="flex items-center gap-1.5 tabular-nums">
            {counts.error > 0 && (
              <span className="text-red-400">{counts.error}</span>
            )}
            {counts.warning > 0 && (
              <span className="text-amber-400">{counts.warning}</span>
            )}
            {counts.info > 0 && (
              <span className="text-sky-400">{counts.info}</span>
            )}
            {running && total === 0 && (
              <span className="text-text-muted/70">…</span>
            )}
          </span>
        )}
      </FooterChipButton>
    </Tooltip>
  );
}
