import { create } from "zustand";
import { TIERS, parseTier, type TierId } from "../billing/tiers.js";

// ---------------------------------------------------------------------------
// Billing store — current subscription + usage snapshot for the signed-in user.
//
// Populated by the app at mount via `refresh()`, which hits /api/usage. The
// ChatUsageMeter reads this for the footer gauge; UpgradeModal reads it to
// show "you've used X of Y". After every chat message the app calls
// refresh() so the meter updates in real time.
// ---------------------------------------------------------------------------

export interface UsageSnapshot {
  tier: TierId;
  /** Start of the current billing / calendar period (UTC ISO). */
  periodStart: string;
  /** End of the current period — when the counter resets (UTC ISO). */
  periodEnd: string;
  inputTokens: number;
  outputTokens: number;
  /** Number of chat messages sent this period — shown in the upgrade modal
   *  as social proof of activity. */
  messageCount: number;
  /** Effective monthly limit for this user's tier. */
  limit: number;
  /** True if the subscription is scheduled to cancel at period end. */
  cancelAtPeriodEnd: boolean;
  /** Subscription status from Stripe (e.g. "active", "past_due"). */
  status: string;
}

export interface BillingState {
  snapshot: UsageSnapshot | null;
  loading: boolean;
  error: string | null;

  setSnapshot: (s: UsageSnapshot | null) => void;
  setLoading: (v: boolean) => void;
  setError: (e: string | null) => void;
  reset: () => void;
}

export const useBillingStore = create<BillingState>((set) => ({
  snapshot: null,
  loading: false,
  error: null,

  setSnapshot: (snapshot) => set({ snapshot, error: null }),
  setLoading: (loading) => set({ loading }),
  setError: (error) => set({ error }),
  reset: () => set({ snapshot: null, loading: false, error: null }),
}));

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

/** Total tokens consumed in the current period. */
export function totalTokensUsed(s: UsageSnapshot): number {
  return s.inputTokens + s.outputTokens;
}

/** 0..1 fraction of the limit consumed. Clamped to the [0, 1.1] range so a
 *  user slightly over the limit still renders a full bar without overflow. */
export function usageFraction(s: UsageSnapshot): number {
  if (s.limit <= 0) return 0;
  return Math.min(1.1, totalTokensUsed(s) / s.limit);
}

/** Heuristic severity used to color the meter and decide whether to nag. */
export function usageSeverity(s: UsageSnapshot): "ok" | "warn" | "critical" {
  const f = usageFraction(s);
  if (f >= 1.0) return "critical";
  if (f >= 0.8) return "warn";
  return "ok";
}

/** Normalize a raw API payload into a UsageSnapshot. Defensive against field
 *  renames and missing keys so a stale client doesn't crash on a new server. */
export function parseUsageResponse(raw: unknown): UsageSnapshot | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const tierRaw = typeof r.tier === "string" ? r.tier : "free";
  const tier = parseTier(tierRaw);
  const limit =
    typeof r.limit === "number" ? r.limit : TIERS[tier].monthlyTokenLimit;
  return {
    tier,
    periodStart: typeof r.periodStart === "string" ? r.periodStart : new Date().toISOString(),
    periodEnd: typeof r.periodEnd === "string" ? r.periodEnd : new Date().toISOString(),
    inputTokens: typeof r.inputTokens === "number" ? r.inputTokens : 0,
    outputTokens: typeof r.outputTokens === "number" ? r.outputTokens : 0,
    messageCount: typeof r.messageCount === "number" ? r.messageCount : 0,
    limit,
    cancelAtPeriodEnd: r.cancelAtPeriodEnd === true,
    status: typeof r.status === "string" ? r.status : "active",
  };
}
