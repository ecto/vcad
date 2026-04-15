// Shared tier configuration — single source of truth for both the server-side
// entitlement check and the client-side usage meter / upgrade modal. Pricing,
// limits, and Stripe price lookup keys all live here so there's never a skew
// between "what the UI shows" and "what the server enforces".
//
// Stripe products are created manually in the dashboard with matching
// `price.lookup_key` values. The webhook maps a Stripe price back to a tier
// via `stripeLookupKey`, and the Checkout endpoint resolves the reverse.

export type TierId = "anon" | "free" | "pro" | "max";

export type PaidTierId = Exclude<TierId, "anon" | "free">;

export interface Tier {
  id: TierId;
  /** Display name shown in the UI. */
  name: string;
  /** Short marketing blurb for the upgrade modal. */
  tagline: string;
  /** Monthly price in USD. `null` for anon (can't purchase). */
  priceMonthlyUsd: number | null;
  /** Monthly token budget (input + output summed). Zero for anon. */
  monthlyTokenLimit: number;
  /** Anon tier only: max messages per rolling 24h window. */
  anonDailyMessageLimit?: number;
  /** Stripe price lookup_key — set manually in the Stripe dashboard. */
  stripeLookupKey?: string;
  /** Bullet-point feature list for the upgrade modal. */
  perks: string[];
}

export const TIERS: Record<TierId, Tier> = {
  anon: {
    id: "anon",
    name: "Anonymous",
    tagline: "Try vcad without signing in.",
    priceMonthlyUsd: null,
    monthlyTokenLimit: 0,
    anonDailyMessageLimit: 3,
    perks: ["3 free chat messages per day", "No account required"],
  },
  free: {
    id: "free",
    name: "Free",
    tagline: "For hobbyists and evaluation.",
    priceMonthlyUsd: 0,
    monthlyTokenLimit: 500_000,
    perks: [
      "500,000 chat tokens per month",
      "All CAD tools & formats",
      "Cloud sync & version history",
    ],
  },
  pro: {
    id: "pro",
    name: "Pro",
    tagline: "For serious makers and consultants.",
    priceMonthlyUsd: 20,
    monthlyTokenLimit: 5_000_000,
    stripeLookupKey: "vcad_pro_monthly",
    perks: [
      "5,000,000 chat tokens per month",
      "Priority model access",
      "Everything in Free",
    ],
  },
  max: {
    id: "max",
    name: "Max",
    tagline: "For studios and power users.",
    priceMonthlyUsd: 100,
    monthlyTokenLimit: 30_000_000,
    stripeLookupKey: "vcad_max_monthly",
    perks: [
      "30,000,000 chat tokens per month",
      "Early access to new features",
      "Everything in Pro",
    ],
  },
};

export const PURCHASABLE_TIERS: PaidTierId[] = ["pro", "max"];

export function getTier(id: TierId): Tier {
  return TIERS[id];
}

/** Resolve a Stripe price lookup_key back to a tier id. */
export function tierFromStripeLookupKey(key: string | null | undefined): PaidTierId | null {
  if (!key) return null;
  for (const id of PURCHASABLE_TIERS) {
    if (TIERS[id].stripeLookupKey === key) return id;
  }
  return null;
}

/** Parse a tier string from the database, falling back to "free". */
export function parseTier(raw: string | null | undefined): TierId {
  if (raw === "pro" || raw === "max" || raw === "free" || raw === "anon") return raw;
  return "free";
}

/** Format a token count as "1.2M" / "500k" / "1,234" for UI display. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return `${m.toFixed(m < 10 ? 1 : 0).replace(/\.0$/, "")}M`;
  }
  if (n >= 1_000) {
    return `${Math.round(n / 1_000)}k`;
  }
  return n.toLocaleString();
}
