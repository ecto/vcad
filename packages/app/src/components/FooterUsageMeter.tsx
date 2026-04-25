import { useEffect, useState } from "react";
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
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/shadcn/hover-card";
import { UpgradeModal } from "@/components/UpgradeModal";
import { cn } from "@/lib/utils";
import { refreshUsage } from "@/lib/billing-api";

/**
 * Compact usage meter docked in the StatusBar footer. Renders the signed-in
 * user's tier and current-period token usage as a slim inline pill. Hovering
 * reveals an expanded breakdown (input/output split, reset date, severity);
 * clicking opens the UpgradeModal.
 *
 * Refreshes on mount, on auth changes, and after every chat streaming turn.
 */
export function FooterUsageMeter() {
  const { isAuthenticated } = useAuth();
  const snapshot = useBillingStore((s) => s.snapshot);
  const loading = useBillingStore((s) => s.loading);
  const streaming = useChatStore((s) => s.streaming);
  const [upgradeOpen, setUpgradeOpen] = useState(false);

  useEffect(() => {
    if (isAuthenticated) void refreshUsage();
    else useBillingStore.getState().reset();
  }, [isAuthenticated]);

  useEffect(() => {
    if (!streaming && isAuthenticated) void refreshUsage();
  }, [streaming, isAuthenticated]);

  if (!isAuthenticated) return null;
  if (!snapshot && !loading) return null;
  if (!snapshot) return null;

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

  const tierTextColor =
    severity === "critical"
      ? "text-danger"
      : severity === "warn"
        ? "text-amber-500"
        : "text-text-muted";

  const statusLabel =
    severity === "critical"
      ? "Limit reached"
      : severity === "warn"
        ? "Approaching limit"
        : null;

  const pct = Math.min(100, frac * 100);

  return (
    <>
      <HoverCard openDelay={120} closeDelay={80}>
        <HoverCardTrigger asChild>
          <button
            type="button"
            onClick={() => setUpgradeOpen(true)}
            aria-label={`${tier.name} plan — ${formatTokens(used)} of ${formatTokens(snapshot.limit)} tokens used. Click to upgrade.`}
            className={cn(
              "group flex items-center gap-2 px-3 border-l border-border/40",
              "text-text-muted hover:text-text hover:bg-hover transition-colors",
              "focus:outline-none focus-visible:bg-hover",
            )}
          >
            <span
              className={cn(
                "uppercase tracking-wide font-medium transition-colors",
                tierTextColor,
                "group-hover:text-text",
              )}
            >
              {tier.name}
            </span>
            <div
              className={cn(
                "h-1 w-12 overflow-hidden bg-border/40",
                "transition-all duration-150 group-hover:w-16",
              )}
              aria-hidden
            >
              <div
                className={cn("h-full transition-all duration-500 ease-out", barColor)}
                style={{ width: `${pct}%` }}
              />
            </div>
            <span className="tabular-nums">
              {formatTokens(used)}
              <span className="text-text-muted/60">/{formatTokens(snapshot.limit)}</span>
            </span>
            {statusLabel && (
              <span
                className={cn(
                  "uppercase tracking-wide hidden md:inline",
                  severity === "critical" ? "text-danger" : "text-amber-500",
                )}
              >
                {statusLabel}
              </span>
            )}
          </button>
        </HoverCardTrigger>
        <HoverCardContent
          side="top"
          align="end"
          sideOffset={6}
          className={cn(
            "w-64 p-3 font-mono text-[11px] border-border bg-surface",
          )}
        >
          <div className="flex items-center justify-between gap-2">
            <span
              className={cn(
                "uppercase tracking-wider font-semibold",
                tierTextColor,
              )}
            >
              {tier.name} plan
            </span>
            <span className="tabular-nums text-text-muted">
              {Math.round(frac * 100)}%
            </span>
          </div>

          <div className="mt-2 h-[5px] w-full overflow-hidden bg-border/40">
            <div
              className={cn("h-full transition-all duration-500 ease-out", barColor)}
              style={{ width: `${pct}%` }}
            />
          </div>

          <div className="mt-2 flex items-center justify-between gap-2 tabular-nums">
            <span className="text-text">
              {formatTokens(used)}{" "}
              <span className="text-text-muted">
                of {formatTokens(snapshot.limit)}
              </span>
            </span>
            {resetLabel && (
              <span className="text-text-muted">resets {resetLabel}</span>
            )}
          </div>

          <div className="mt-2 grid grid-cols-2 gap-2 text-text-muted">
            <div className="flex flex-col">
              <span className="text-[9px] uppercase tracking-wider opacity-70">
                input
              </span>
              <span className="tabular-nums text-text">
                {formatTokens(snapshot.inputTokens)}
              </span>
            </div>
            <div className="flex flex-col">
              <span className="text-[9px] uppercase tracking-wider opacity-70">
                output
              </span>
              <span className="tabular-nums text-text">
                {formatTokens(snapshot.outputTokens)}
              </span>
            </div>
          </div>

          {statusLabel && (
            <div
              className={cn(
                "mt-2 uppercase tracking-wider text-[9px] font-semibold",
                severity === "critical" ? "text-danger" : "text-amber-500",
              )}
            >
              {statusLabel}
            </div>
          )}

          <button
            type="button"
            onClick={() => setUpgradeOpen(true)}
            className={cn(
              "mt-3 block w-full px-2 py-1 text-center text-[10px] uppercase tracking-wider font-semibold",
              "bg-brand/10 text-brand hover:bg-brand hover:text-white transition-colors",
            )}
          >
            Upgrade →
          </button>
        </HoverCardContent>
      </HoverCard>

      <UpgradeModal
        open={upgradeOpen}
        onOpenChange={setUpgradeOpen}
        reason="manual"
      />
    </>
  );
}
