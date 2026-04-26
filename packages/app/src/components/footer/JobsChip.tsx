import { X } from "@phosphor-icons/react/dist/ssr/X";
import { useJobsStore } from "@vcad/core";
import { FooterChip } from "@/components/footer/FooterChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/**
 * Active jobs surface in the footer.
 *
 * When at least one job is in flight, this chip slides in showing the most
 * recent job's verb plus a thin progress bar (indeterminate when the job
 * doesn't report progress). Cancellable jobs grow an X button.
 *
 * Multiple concurrent jobs collapse to a "+N more" suffix; the latest one
 * drives the chip surface so the user always sees the most recent work.
 */
export function JobsChip({ className }: { className?: string }) {
  const jobs = useJobsStore((s) => s.jobs);
  const requestCancel = useJobsStore((s) => s.requestCancel);

  if (jobs.length === 0) return null;

  // Drive the chip from the most recent job.
  const job = jobs[jobs.length - 1]!;
  const others = jobs.length - 1;

  const indeterminate = job.progress === null;
  const pct = indeterminate ? 0 : Math.max(0, Math.min(100, job.progress! * 100));

  return (
    <Tooltip
      side="top"
      content={`Long-running operation in flight${others > 0 ? ` (+${others} more queued)` : ""} — verb: ${job.verb}`}
    >
    <FooterChip
      severity="brand"
      className={cn(
        "animate-in fade-in slide-in-from-right-2 duration-200",
        "gap-2",
        className,
      )}
    >
      <span className="uppercase tracking-wide text-text-muted">
        {job.verb}
      </span>

      <div
        className="h-1 w-16 overflow-hidden bg-border/40 rounded-sm"
        aria-hidden
      >
        {indeterminate ? (
          <div className="vcad-job-indeterminate h-full w-1/3 bg-brand" />
        ) : (
          <div
            className="h-full bg-brand transition-all duration-150 ease-out"
            style={{ width: `${pct}%` }}
          />
        )}
      </div>

      {!indeterminate && (
        <span className="tabular-nums text-text-muted/70 w-8 text-right">
          {pct.toFixed(0)}%
        </span>
      )}

      {others > 0 && (
        <span className="text-text-muted/60">+{others}</span>
      )}

      {job.cancellable && !job.cancelRequested && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            requestCancel(job.id);
          }}
          className="text-text-muted/70 hover:text-danger transition-colors"
          aria-label={`Cancel ${job.verb}`}
        >
          <X size={10} weight="bold" />
        </button>
      )}
      {job.cancelRequested && (
        <span className="text-text-muted/60 italic">cancelling…</span>
      )}
    </FooterChip>
    </Tooltip>
  );
}
