/**
 * Order-dock state chips — the fused vcad OrderState × kerf JobState → chip
 * mapping from docs/agent-native-factory.md (M2). Both maps are exhaustive
 * switches with never-checks so a new state on either side breaks the build
 * here instead of reaching the widget unmapped.
 */

import type { JobState } from "./kerf/contract.js";
import type { AuthorizationStatus, OrderState } from "./types.js";

/** The six-stop dock timeline (+ failed). */
export type OrderChip =
  | "quoted"
  | "approval"
  | "placing"
  | "confirmed"
  | "production"
  | "delivered"
  | "failed";

/**
 * Chip for a vcad order state. A QUOTED order with a pending_human spend
 * authorization renders as "approval" — the human's next move, not the
 * order's last one, is what the dock foregrounds.
 */
export function orderStateChip(
  state: OrderState,
  authzStatus?: AuthorizationStatus,
): OrderChip {
  switch (state) {
    case "DRAFT":
      return "quoted";
    case "QUOTED":
      return authzStatus === "pending_human" ? "approval" : "quoted";
    case "AUTHORIZED":
    case "PENDING_PAYMENT":
      return "approval";
    case "PAID":
    case "SUBMITTED":
    case "RECONCILING":
      return "placing";
    case "IN_PRODUCTION":
    case "SHIPPED":
      return "production";
    case "DELIVERED":
      return "delivered";
    case "EXPIRED":
    case "PAYMENT_FAILED":
    case "SUBMIT_FAILED":
    case "CANCELED":
    case "CANCELED_BY_FAB":
    case "REFUNDED":
      return "failed";
    default: {
      const exhaustive: never = state;
      return exhaustive;
    }
  }
}

/**
 * Chip for a kerf job state (all 17). RECONCILING renders as "placing" — the
 * dock explains it as a wait ("click outcome ambiguous, checking vendor
 * history"), never a retry affordance; raw states stay in the event log.
 */
export function kerfJobChip(state: JobState): OrderChip {
  switch (state) {
    case "QUEUED":
    case "SESSION_OPEN":
    case "STAGING":
    case "STAGED":
    case "AUDIT":
    case "PLACING":
    case "CONFIRMING":
    case "RECONCILING":
    case "RECONCILED_PLACED":
      return "placing";
    case "TAKEOVER_WAIT":
      return "approval";
    case "CONFIRMED":
      return "confirmed";
    case "TRACKING":
      return "production";
    case "DELIVERED":
      return "delivered";
    case "AUDIT_FAILED":
    case "FAILED":
    case "RECONCILED_ABSENT":
    case "CANCELED":
      return "failed";
    default: {
      const exhaustive: never = state;
      return exhaustive;
    }
  }
}
