import { useEffect } from "react";
import {
  useBillingStore,
  useChatStore,
  TIERS,
  formatTokens,
  totalTokensUsed,
  usageFraction,
  usageSeverity,
} from "@vcad/core";
import { useAuth } from "@vcad/auth";
import { cn } from "@/lib/utils";
import { refreshUsage } from "@/lib/billing-api";

interface ChatUsageMeterProps {
  /** Opens the upgrade modal when the user clicks Upgrade / the warning. */
  onUpgradeClick: () => void;
}

/**
 * Thin usage meter rendered in the ChatSidebar footer. Shows the signed-in
 * user's current period consumption as a horizontal bar + compact numbers.
 * Transitions through three severities:
 *   ok       — neutral
 *   warn     — amber, surfaces at ≥80% used
 *   critical — red, ≥100% used, with inline "Upgrade" button
 *
 * Refreshes on mount, on auth changes, and whenever a chat message finishes
 * streaming (so the bar fills live as tokens are consumed).
 */
export function ChatUsageMeter({ onUpgradeClick }: ChatUsageMeterProps) {
  const { isAuthenticated } = useAuth();
  const snapshot = useBillingStore((s) => s.snapshot);
  const loading = useBillingStore((s) => s.loading);
  const streaming = useChatStore((s) => s.streaming);

  // Initial load + refresh when the user signs in / out.
  useEffect(() => {
    if (isAuthenticated) void refreshUsage();
    else useBillingStore.getState().reset();
  }, [isAuthenticated]);

  // Refresh at the end of every streaming turn so the meter reflects the
  // tokens just consumed.
  useEffect(() => {
    if (!streaming && isAuthenticated) void refreshUsage();
  }, [streaming, isAuthenticated]);

  // Anon (or anonymous Supabase session) — the chat sidebar surfaces the
  // free-tier counter elsewhere; this billing meter is paid-tier UX.
  if (!isAuthenticated) return null;
  if (!snapshot && !loading) return null;
  if (!snapshot) {
    // First fetch in progress — reserve layout space with a thin ghost.
    return <div className="h-6" aria-hidden />;
  }

  const used = totalTokensUsed(snapshot);
  const frac = usageFraction(snapshot);
  const severity = usageSeverity(snapshot);
  const tier = TIERS[snapshot.tier];

  const resetLabel = (() => {
    try {
      const d = new Date(snapshot.periodEnd);
      return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
    } catch {
      return null;
    }
  })();

  const barColor =
    severity === "critical"
      ? "bg-danger"
      : severity === "warn"
        ? "bg-amber-500"
        : "bg-brand";

  const textColor =
    severity === "critical"
      ? "text-danger"
      : severity === "warn"
        ? "text-amber-500"
        : "text-text-muted";

  // Tooltip surfaces the "this opens the upgrade modal" affordance on hover.
  const title =
    severity === "critical"
      ? "Monthly limit reached — click to upgrade"
      : severity === "warn"
        ? "Approaching monthly limit — click to upgrade"
        : `${tier.name} plan — click to view or upgrade`;

  const statusLabel =
    severity === "critical"
      ? "Limit reached"
      : severity === "warn"
        ? "Approaching limit"
        : null;

  return (
    <button
      type="button"
      onClick={onUpgradeClick}
      title={title}
      aria-label={title}
      className={cn(
        "group block w-full shrink-0 cursor-pointer px-3 pt-1.5 pb-1 text-left",
        "transition-colors duration-150 hover:bg-hover/60",
        "focus-visible:outline focus-visible:outline-1 focus-visible:-outline-offset-1 focus-visible:outline-brand",
      )}
    >
      <div className="flex items-center justify-between gap-2 text-[9px] leading-none">
        <span className={cn("font-mono uppercase tracking-wider transition-colors", textColor)}>
          {tier.name}
        </span>
        <span
          className={cn(
            "font-mono transition-colors",
            textColor,
            "group-hover:text-text",
          )}
        >
          {formatTokens(used)} / {formatTokens(snapshot.limit)}
          {resetLabel && (
            <span className="text-text-muted/60"> · resets {resetLabel}</span>
          )}
        </span>
      </div>
      <div
        className={cn(
          "mt-1 h-[3px] w-full overflow-hidden bg-border/40 transition-[height] duration-150",
          "group-hover:h-[5px]",
        )}
      >
        <div
          className={cn("h-full transition-all duration-500 ease-out", barColor)}
          style={{ width: `${Math.min(100, frac * 100)}%` }}
        />
      </div>
      <div className="mt-1 flex items-center justify-between gap-2 text-[9px] leading-none">
        <span
          className={cn(
            "font-mono uppercase tracking-wider transition-colors",
            severity === "critical"
              ? "text-danger"
              : severity === "warn"
                ? "text-amber-500"
                : "text-text-muted/60 group-hover:text-text-muted",
          )}
        >
          {statusLabel ?? "Click to upgrade"}
        </span>
        <span
          className={cn(
            "font-mono uppercase tracking-wider transition-colors",
            severity === "critical"
              ? "text-danger group-hover:text-text"
              : severity === "warn"
                ? "text-amber-500 group-hover:text-text"
                : "text-text-muted/60 group-hover:text-brand",
          )}
        >
          Upgrade →
        </span>
      </div>
    </button>
  );
}
