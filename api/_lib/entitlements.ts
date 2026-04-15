// Entitlements: map a user id to their effective tier + current billing
// period + monthly token limit, then record usage atomically.
//
// Design notes:
//  * Paid users' period is Stripe's current_period_start/end, synced by the
//    webhook. Free users' period is the calendar month UTC.
//  * `past_due` and `unpaid` are treated as "still entitled" through the end
//    of the already-paid period (Stripe's current_period_end). After that,
//    the webhook flips them to canceled and they fall back to free.
//  * Rate limit check is a single indexed PK lookup on usage_periods — no
//    sum-of-logs scan.

import type { SupabaseClient } from "@supabase/supabase-js";
import { TIERS, parseTier, type TierId } from "@vcad/core";

export interface Entitlement {
  tier: TierId;
  /** Effective monthly token budget for this user. */
  limit: number;
  /** Start of the current usage window (UTC). */
  periodStart: Date;
  /** End of the current usage window (UTC). */
  periodEnd: Date;
  cancelAtPeriodEnd: boolean;
  status: string;
}

export interface PeriodUsage {
  inputTokens: number;
  outputTokens: number;
  messageCount: number;
}

interface SubscriptionRow {
  tier: string;
  status: string;
  current_period_start: string | null;
  current_period_end: string | null;
  cancel_at_period_end: boolean;
}

/** Start of the current calendar month, UTC. Used as the free tier period. */
function startOfCalendarMonthUtc(now: Date = new Date()): Date {
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 1, 0, 0, 0));
}

/** Start of the next calendar month, UTC. Used as the free tier period end. */
function startOfNextCalendarMonthUtc(now: Date = new Date()): Date {
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() + 1, 1, 0, 0, 0));
}

function defaultFreeEntitlement(now: Date = new Date()): Entitlement {
  return {
    tier: "free",
    limit: TIERS.free.monthlyTokenLimit,
    periodStart: startOfCalendarMonthUtc(now),
    periodEnd: startOfNextCalendarMonthUtc(now),
    cancelAtPeriodEnd: false,
    status: "active",
  };
}

/**
 * Resolve a user's current entitlement by reading their `subscriptions` row.
 * Users with no row, or with an expired/canceled sub, fall back to the free
 * tier anchored to the calendar month.
 */
export async function getEntitlement(
  admin: SupabaseClient,
  userId: string,
): Promise<Entitlement> {
  const now = new Date();
  const { data, error } = await admin
    .from("subscriptions")
    .select("tier, status, current_period_start, current_period_end, cancel_at_period_end")
    .eq("user_id", userId)
    .maybeSingle();
  if (error) {
    console.error("[entitlements] lookup failed:", error);
    return defaultFreeEntitlement(now);
  }
  if (!data) return defaultFreeEntitlement(now);

  const row = data as SubscriptionRow;
  const tier = parseTier(row.tier);

  // An active/trialing sub always grants its tier. past_due / unpaid grant
  // the tier until the period end (grace through the period Stripe already
  // billed for). Everything else degrades to free.
  const nowMs = now.getTime();
  const periodEnd = row.current_period_end ? new Date(row.current_period_end) : null;
  const periodStart = row.current_period_start ? new Date(row.current_period_start) : null;

  const grantsTier = (() => {
    if (tier === "free") return true;
    if (row.status === "active" || row.status === "trialing") return true;
    if (row.status === "past_due" || row.status === "unpaid") {
      return periodEnd !== null && periodEnd.getTime() > nowMs;
    }
    return false;
  })();

  if (!grantsTier || !periodStart || !periodEnd) {
    return defaultFreeEntitlement(now);
  }

  return {
    tier,
    limit: TIERS[tier].monthlyTokenLimit,
    periodStart,
    periodEnd,
    cancelAtPeriodEnd: row.cancel_at_period_end,
    status: row.status,
  };
}

/**
 * Read the current period's usage for a user. Returns zeros if no row yet
 * (user hasn't sent a message this period).
 */
export async function getPeriodUsage(
  admin: SupabaseClient,
  userId: string,
  periodStart: Date,
): Promise<PeriodUsage> {
  const { data, error } = await admin
    .from("usage_periods")
    .select("input_tokens, output_tokens, message_count")
    .eq("user_id", userId)
    .eq("period_start", periodStart.toISOString())
    .maybeSingle();
  if (error) {
    console.error("[entitlements] usage lookup failed:", error);
    return { inputTokens: 0, outputTokens: 0, messageCount: 0 };
  }
  if (!data) return { inputTokens: 0, outputTokens: 0, messageCount: 0 };
  return {
    inputTokens: Number(data.input_tokens ?? 0),
    outputTokens: Number(data.output_tokens ?? 0),
    messageCount: Number(data.message_count ?? 0),
  };
}

/**
 * Atomically record token usage for the current period. Uses the
 * `record_chat_usage` RPC which performs an UPSERT + increment in one
 * statement, so two concurrent requests from the same user race on a single
 * row update instead of corrupting the counter.
 */
export async function recordChatUsage(
  admin: SupabaseClient,
  userId: string,
  entitlement: Entitlement,
  tokens: { input: number; output: number },
): Promise<void> {
  const { error } = await admin.rpc("record_chat_usage", {
    p_user_id: userId,
    p_period_start: entitlement.periodStart.toISOString(),
    p_period_end: entitlement.periodEnd.toISOString(),
    p_tier: entitlement.tier,
    p_input_tokens: tokens.input,
    p_output_tokens: tokens.output,
  });
  if (error) {
    console.error("[entitlements] record_chat_usage failed:", error);
  }
}

/** True if the user has already consumed their monthly budget. */
export function isOverLimit(entitlement: Entitlement, usage: PeriodUsage): boolean {
  const total = usage.inputTokens + usage.outputTokens;
  return total >= entitlement.limit;
}
