/**
 * Daily quote-only canaries — the drift alarm that turns "brittle" into a
 * measured SLO.
 *
 * For every registry vendor with a `canary` spec: run the quote workflow
 * with that vendor's fixture intent (budget_minor is typed as literal 0 —
 * canaries never spend), then update the capability's status/freshness:
 * green on success, red on failure (which auto-opens a Tier-2 repair job),
 * amber when stale. Freshness is surfaced to clients alongside quotes
 * ("sendcutsend: green, verified 6h ago").
 *
 * TODO(bringup): express as an eve schedule per node_modules/eve/docs
 * (agent/schedules convention); the body is quoteJob() over the registry.
 */

export const CANARY_SCHEDULE_PLACEHOLDER = {
  cadence: "daily",
  spends_money: false as const,
  on_failure: "open Tier-2 repair job + mark capability red",
};
