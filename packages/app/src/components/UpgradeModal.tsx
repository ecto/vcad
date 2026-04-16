import { useEffect, useMemo, useState, type ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { ArrowRight } from "@phosphor-icons/react/dist/ssr/ArrowRight";
import { LockSimple } from "@phosphor-icons/react/dist/ssr/LockSimple";
import { Lightning } from "@phosphor-icons/react/dist/ssr/Lightning";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { Infinity as InfinityIcon } from "@phosphor-icons/react/dist/ssr/Infinity";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import {
  TIERS,
  PURCHASABLE_TIERS,
  formatTokens,
  totalTokensUsed,
  useBillingStore,
  type PaidTierId,
  type TierId,
  type UsageSnapshot,
} from "@vcad/core";
import { cn } from "@/lib/utils";
import { startCheckout, openCustomerPortal, refreshUsage } from "@/lib/billing-api";

interface UpgradeModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** "limit-reached" shows the focused roadblock; "manual" shows the grid. */
  reason?: "limit-reached" | "manual";
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

function humanUntil(iso: string): { label: string; compact: string } {
  const ms = new Date(iso).getTime() - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) {
    return { label: "any moment", compact: "soon" };
  }
  const days = Math.floor(ms / 86_400_000);
  if (days >= 2) return { label: `in ${days} days`, compact: `${days}d` };
  if (days === 1) return { label: "tomorrow", compact: "1d" };
  const hours = Math.max(1, Math.floor(ms / 3_600_000));
  return { label: `in ${hours}h`, compact: `${hours}h` };
}

function formatResetDate(iso: string): string {
  try {
    return new Date(iso).toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
    });
  } catch {
    return "soon";
  }
}

/** Daily-price framing is the single strongest anchor in consumer SaaS copy —
 *  "67¢/day" reads as cheap where "$20/month" can read as expensive. */
function dailyPrice(monthlyUsd: number): string {
  const per = monthlyUsd / 30;
  return per >= 1 ? `$${per.toFixed(2)}/day` : `${Math.round(per * 100)}¢/day`;
}

// Icons associated with each perk position. The underlying `perks` list is
// the source of truth (defined in @vcad/core); we overlay meaning-specific
// glyphs on top so the modal reads like a product page, not a bulleted doc.
const PERK_ICONS = [InfinityIcon, Lightning, Sparkle, Cube];

// ---------------------------------------------------------------------------
// Tier comparison bar — the centerpiece of the limit-reached view.
//
// Shows two bars normalized so the larger tier fills the container:
//   row 1: current plan, fully red, "100% used" label
//   row 2: target plan, with the user's SAME usage as a small filled slice
//          on top of a large unfilled capacity
//
// The emotional punch is "with Pro, that exact same month barely dents the
// bar." No text can convey that as fast as two side-by-side bars.
// ---------------------------------------------------------------------------

function TierComparisonBar({
  currentTier,
  targetTier,
  used,
}: {
  currentTier: TierId;
  targetTier: PaidTierId;
  used: number;
}) {
  const current = TIERS[currentTier];
  const target = TIERS[targetTier];
  // Both bars span the full container width. The emotional payload comes
  // from the fill *percentage*, not the bar length:
  //   current → 100% filled red (you've used every drop)
  //   target  → tiny slice filled (same usage, huge room left)
  const currentFillPct = Math.min(
    100,
    (used / Math.max(current.monthlyTokenLimit, 1)) * 100,
  );
  const targetFillFrac = Math.min(1, used / target.monthlyTokenLimit);
  // Floor the target fill so even a 2% slice is visually present instead
  // of vanishing into the bar background.
  const targetFillPct = Math.max(2, targetFillFrac * 100);
  const multiple = Math.round(target.monthlyTokenLimit / current.monthlyTokenLimit);

  return (
    <div className="border border-border bg-bg/40 px-4 py-4">
      {/* Current plan bar — full width, fully red */}
      <div className="flex items-center justify-between gap-2 text-[9px] font-semibold uppercase tracking-[0.12em]">
        <span className="text-text-muted">
          Your month on {current.name}
        </span>
        <span className="tabular-nums text-danger">
          {formatTokens(used)} / {formatTokens(current.monthlyTokenLimit)}
        </span>
      </div>
      <div className="mt-1.5 h-2 w-full bg-border/40">
        <div
          className="h-full bg-danger transition-[width] duration-700 ease-out"
          style={{ width: `${currentFillPct}%` }}
        />
      </div>

      {/* Divider — small air between the two rows */}
      <div className="mt-4" />

      {/* Target plan bar — full width, same usage as a tiny slice */}
      <div className="flex items-center justify-between gap-2 text-[9px] font-semibold uppercase tracking-[0.12em]">
        <span className="text-brand">Same month on {target.name}</span>
        <span className="tabular-nums text-brand">
          {formatTokens(used)} / {formatTokens(target.monthlyTokenLimit)}
        </span>
      </div>
      <div className="mt-1.5 h-2 w-full bg-brand/15">
        <div
          className="h-full bg-brand transition-[width] duration-700 ease-out delay-150"
          style={{ width: `${targetFillPct}%` }}
        />
      </div>
      <div className="mt-1.5 text-[9px] text-text-muted">
        {formatTokens(target.monthlyTokenLimit - used)} tokens remaining ·{" "}
        <span className="text-brand">{multiple}× the room</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Limit-reached view — the roadblock. Hero + comparison bar + single CTA.
// ---------------------------------------------------------------------------

interface LimitReachedViewProps {
  snapshot: UsageSnapshot;
  busy: PaidTierId | "portal" | null;
  onCheckout: (tier: PaidTierId) => void;
  onDismiss: () => void;
}

function LimitReachedView({
  snapshot,
  busy,
  onCheckout,
  onDismiss,
}: LimitReachedViewProps) {
  const used = totalTokensUsed(snapshot);
  const currentTier = TIERS[snapshot.tier];

  const reset = useMemo(() => humanUntil(snapshot.periodEnd), [snapshot.periodEnd]);
  const resetDate = formatResetDate(snapshot.periodEnd);

  // Headline recommendation: if the user is already on Pro, push Max.
  // Otherwise push Pro. There's always exactly one primary CTA.
  const recommendedId: PaidTierId = snapshot.tier === "pro" ? "max" : "pro";
  const recommended = TIERS[recommendedId];
  const max = TIERS.max;
  const multiple =
    currentTier.monthlyTokenLimit > 0
      ? Math.round(recommended.monthlyTokenLimit / currentTier.monthlyTokenLimit)
      : null;

  return (
    <>
      {/* Red "monthly limit reached" tag — honest header, then warmer copy. */}
      <div className="flex items-center gap-2 px-7 pt-6">
        <span
          aria-hidden
          className="inline-block h-1.5 w-1.5 animate-pulse bg-danger"
        />
        <span className="font-mono text-[9px] font-semibold uppercase tracking-[0.12em] text-danger">
          Monthly limit reached
        </span>
      </div>

      <div className="px-7 pt-3 pb-5">
        <Dialog.Title className="text-[22px] font-bold leading-tight tracking-tighter text-text">
          Keep the momentum going.
        </Dialog.Title>
        <Dialog.Description className="mt-2 text-[11px] leading-relaxed text-text-muted">
          You've used every token on the{" "}
          <span className="text-text">{currentTier.name}</span> plan this
          month. Upgrade to finish what you started
          {multiple ? ` — ${multiple}× more tokens, ` : " — "}
          back to building in under a minute.
        </Dialog.Description>
      </div>

      {/* Visual comparison bar — the hero. */}
      <div className="mx-7 mb-5">
        <TierComparisonBar
          currentTier={snapshot.tier}
          targetTier={recommendedId}
          used={used}
        />
        <div className="mt-2 flex items-center justify-between text-[9px] text-text-muted">
          <span>
            <span className="text-text tabular-nums">
              {snapshot.messageCount.toLocaleString()}
            </span>{" "}
            {snapshot.messageCount === 1 ? "message" : "messages"} this month
          </span>
          <span>
            Resets {reset.label} ·{" "}
            <span className="text-text">{resetDate}</span>
          </span>
        </div>
      </div>

      {/* Primary CTA card — Pro (or Max if already Pro). Single dominant
          action with all the conversion levers in one place. */}
      <div className="mx-7 mb-3">
        <div className="relative border border-brand/70 bg-brand/[0.06] p-5 ring-1 ring-brand/20">
          <div className="absolute -top-[9px] left-4 flex items-center gap-1 bg-brand px-1.5 py-0.5 font-mono text-[9px] font-bold uppercase tracking-[0.12em] text-white">
            <Lightning size={9} weight="fill" />
            Recommended
          </div>

          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <div className="flex items-baseline gap-2">
                <h3 className="text-[15px] font-bold tracking-tight text-text">
                  {recommended.name}
                </h3>
                <span className="truncate text-[10px] text-text-muted">
                  {recommended.tagline}
                </span>
              </div>
              <div className="mt-1 text-[10px] text-text-muted">
                {formatTokens(recommended.monthlyTokenLimit)} tokens
                {multiple ? ` · ${multiple}× ${currentTier.name}` : ""}
                {" · "}
                <span className="text-text">
                  {dailyPrice(recommended.priceMonthlyUsd ?? 0)}
                </span>
              </div>
            </div>
            <div className="shrink-0 text-right">
              <div className="flex items-baseline gap-1 justify-end">
                <span className="text-[28px] font-bold leading-none tracking-tighter text-text">
                  ${recommended.priceMonthlyUsd}
                </span>
                <span className="text-[10px] text-text-muted">/mo</span>
              </div>
            </div>
          </div>

          {/* Icon-backed perks — visual + label instead of flat bullets. */}
          <div className="mt-4 grid grid-cols-1 gap-2">
            {recommended.perks.map((perk, i) => {
              const Icon = PERK_ICONS[i] ?? Check;
              return (
                <PerkRow key={perk} icon={<Icon size={12} weight="bold" />}>
                  {perk}
                </PerkRow>
              );
            })}
          </div>

          <button
            type="button"
            disabled={busy !== null}
            onClick={() => onCheckout(recommendedId)}
            className={cn(
              "upgrade-cta group mt-5 flex h-11 w-full items-center justify-center gap-2",
              "bg-brand font-mono text-[12px] font-bold uppercase tracking-[0.12em] text-white",
              "transition-[background-color,transform,box-shadow] duration-150",
              "hover:bg-brand-hover hover:shadow-[0_8px_24px_-8px_rgba(249,38,114,0.6)]",
              "active:translate-y-[1px]",
              "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-brand",
              "disabled:cursor-not-allowed disabled:opacity-40",
            )}
          >
            {busy === recommendedId ? (
              <span>Redirecting to checkout…</span>
            ) : (
              <>
                <span>Upgrade to {recommended.name}</span>
                <ArrowRight
                  size={13}
                  weight="bold"
                  className="transition-transform duration-150 group-hover:translate-x-0.5"
                />
              </>
            )}
          </button>
        </div>
      </div>

      {/* Secondary tier — muted link, not a second big button. */}
      {snapshot.tier !== "pro" && (
        <div className="px-7 pb-3 text-center">
          <button
            type="button"
            disabled={busy !== null}
            onClick={() => onCheckout("max")}
            className="font-mono text-[10px] text-text-muted transition-colors hover:text-text disabled:opacity-40"
          >
            Need more? Upgrade to{" "}
            <span className="text-text">{max.name}</span> —{" "}
            {formatTokens(max.monthlyTokenLimit)} tokens · ${max.priceMonthlyUsd}/mo
          </button>
        </div>
      )}

      {/* Trust + soft ejector row. Respects user autonomy. */}
      <div className="mt-2 flex items-center justify-between gap-3 border-t border-border bg-bg/40 px-7 py-3">
        <div className="flex items-center gap-1.5 font-mono text-[9px] text-text-muted">
          <LockSimple size={10} weight="fill" />
          <span>Secure · Stripe · Cancel anytime</span>
        </div>
        <button
          type="button"
          onClick={onDismiss}
          className="font-mono text-[9px] text-text-muted transition-colors hover:text-text"
        >
          I'll wait until {resetDate}
        </button>
      </div>
    </>
  );
}

function PerkRow({
  icon,
  children,
}: {
  icon: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2.5 border border-border/40 bg-bg/40 px-3 py-2">
      <span className="flex h-5 w-5 shrink-0 items-center justify-center bg-brand/15 text-brand">
        {icon}
      </span>
      <span className="text-[11px] text-text">{children}</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Manual view — comparison grid for users who open from the menu / meter.
// ---------------------------------------------------------------------------

interface ManualViewProps {
  snapshot: UsageSnapshot | null;
  busy: PaidTierId | "portal" | null;
  onCheckout: (tier: PaidTierId) => void;
  onPortal: () => void;
}

function ManualView({ snapshot, busy, onCheckout, onPortal }: ManualViewProps) {
  const currentTier = snapshot?.tier ?? "free";
  return (
    <>
      <div className="px-6 pt-6 pb-4">
        <Dialog.Title className="text-lg font-bold tracking-tighter text-text">
          Upgrade your plan
        </Dialog.Title>
        <Dialog.Description className="mt-1 text-xs text-text-muted">
          More tokens, more modeling. Cancel anytime from the customer portal.
        </Dialog.Description>
      </div>

      <div className="grid grid-cols-1 gap-3 px-6 pb-6 sm:grid-cols-2">
        {PURCHASABLE_TIERS.map((tierId) => {
          const tier = TIERS[tierId];
          const isCurrent = currentTier === tierId;
          const isDowngrade = currentTier === "max" && tierId === "pro";
          const isRecommended = currentTier === "free" && tierId === "pro";
          return (
            <div
              key={tierId}
              className={cn(
                "relative flex flex-col border bg-bg/40 p-4 transition-colors",
                isRecommended
                  ? "border-brand/60 ring-1 ring-brand/20"
                  : isCurrent
                    ? "border-brand/60"
                    : "border-border hover:border-text-muted/60",
              )}
            >
              {isRecommended && (
                <div className="absolute -top-[9px] left-4 bg-brand px-1.5 py-0.5 font-mono text-[9px] font-bold uppercase tracking-[0.12em] text-white">
                  Recommended
                </div>
              )}
              <div className="flex items-baseline justify-between">
                <h3 className="text-sm font-bold tracking-tight text-text">
                  {tier.name}
                </h3>
                {isCurrent && (
                  <span className="bg-brand/15 px-1.5 py-0.5 font-mono text-[9px] font-semibold uppercase tracking-wider text-brand">
                    Current
                  </span>
                )}
              </div>
              <div className="mt-1 flex items-baseline gap-1">
                <span className="text-xl font-bold tracking-tighter text-text">
                  ${tier.priceMonthlyUsd}
                </span>
                <span className="text-[10px] text-text-muted">/ month</span>
              </div>
              <p className="mt-1 text-[10px] text-text-muted">
                {formatTokens(tier.monthlyTokenLimit)} tokens ·{" "}
                {dailyPrice(tier.priceMonthlyUsd ?? 0)}
              </p>

              <ul className="mt-3 flex-1 space-y-1.5">
                {tier.perks.map((perk) => (
                  <li
                    key={perk}
                    className="flex items-start gap-1.5 text-[11px] text-text"
                  >
                    <Check
                      size={11}
                      weight="bold"
                      className="mt-[3px] shrink-0 text-brand"
                    />
                    <span>{perk}</span>
                  </li>
                ))}
              </ul>

              <button
                type="button"
                disabled={busy !== null || isCurrent}
                onClick={() =>
                  isCurrent || isDowngrade ? onPortal() : onCheckout(tierId)
                }
                className={cn(
                  "mt-4 h-9 font-mono text-[11px] font-bold uppercase tracking-[0.12em]",
                  "transition-[background-color,transform,box-shadow] duration-150",
                  "disabled:cursor-not-allowed disabled:opacity-40",
                  "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-brand",
                  isCurrent
                    ? "border border-border text-text-muted"
                    : "bg-brand text-white hover:bg-brand-hover hover:shadow-[0_8px_24px_-8px_rgba(249,38,114,0.5)] active:translate-y-[1px]",
                )}
              >
                {busy === tierId
                  ? "Redirecting..."
                  : isCurrent
                    ? "Current plan"
                    : isDowngrade
                      ? "Manage to downgrade"
                      : `Upgrade to ${tier.name}`}
              </button>
            </div>
          );
        })}
      </div>

      {snapshot && snapshot.tier !== "free" && (
        <div className="flex items-center justify-between border-t border-border px-6 py-3">
          <span className="text-[10px] text-text-muted">
            Need to update billing details?
          </span>
          <button
            type="button"
            disabled={busy !== null}
            onClick={onPortal}
            className="text-[10px] text-brand transition-colors hover:text-brand-hover hover:underline disabled:opacity-40"
          >
            {busy === "portal" ? "Opening..." : "Manage subscription →"}
          </button>
        </div>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// UpgradeModal root
// ---------------------------------------------------------------------------

export function UpgradeModal({ open, onOpenChange, reason }: UpgradeModalProps) {
  const snapshot = useBillingStore((s) => s.snapshot);
  const [busy, setBusy] = useState<PaidTierId | "portal" | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Refresh usage snapshot whenever the modal opens — ensures the limit-
  // reached hero shows current numbers even if the billing store is stale.
  useEffect(() => {
    if (open) {
      void refreshUsage();
      setError(null);
      setBusy(null);
    }
  }, [open]);

  const handleCheckout = async (tier: PaidTierId) => {
    setError(null);
    setBusy(tier);
    try {
      const url = await startCheckout(tier);
      // Navigate via a synthetic anchor click. Safari blocks
      // window.location.href in async callbacks that have lost the
      // original user-gesture context; dispatching a click on an <a>
      // avoids that restriction.
      const a = document.createElement("a");
      a.href = url;
      a.rel = "noopener";
      a.click();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Checkout failed");
      setBusy(null);
    }
  };

  const handlePortal = async () => {
    setError(null);
    setBusy("portal");
    try {
      await openCustomerPortal();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Portal failed");
      setBusy(null);
    }
  };

  const isLimitReached = reason === "limit-reached" && snapshot !== null;

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay
          className="upgrade-overlay fixed inset-0 z-50 bg-black/60 backdrop-blur-md"
        />
        <Dialog.Content
          aria-describedby={undefined}
          className={cn(
            "upgrade-content fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2",
            "border border-border bg-card/95 backdrop-blur-md shadow-2xl focus:outline-none",
            isLimitReached
              ? "w-[min(540px,94vw)]"
              : "w-[min(720px,92vw)]",
          )}
        >
          <div className="absolute right-2 top-2 z-10">
            <Dialog.Close
              aria-label="Close"
              className="p-1 text-text-muted transition-colors hover:bg-border/50 hover:text-text cursor-pointer"
            >
              <X size={14} />
            </Dialog.Close>
          </div>

          {error && (
            <div className="mx-6 mt-5 border border-danger/30 bg-danger/10 px-3 py-2 text-[11px] text-danger">
              {error}
            </div>
          )}

          {isLimitReached && snapshot ? (
            <LimitReachedView
              snapshot={snapshot}
              busy={busy}
              onCheckout={handleCheckout}
              onDismiss={() => onOpenChange(false)}
            />
          ) : (
            <ManualView
              snapshot={snapshot}
              busy={busy}
              onCheckout={handleCheckout}
              onPortal={handlePortal}
            />
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
