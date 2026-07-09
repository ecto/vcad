import { describe, it, expect } from "vitest";
import { orderStateChip, kerfJobChip, type OrderChip } from "../fabricate/state-chip.js";
import { KERF_JOB_STATES } from "../fabricate/kerf/contract.js";
import type { OrderState } from "../fabricate/types.js";

/**
 * Exhaustive chip-mapping coverage (M2): every vcad OrderState and every one
 * of the 17 kerf JobStates maps to a dock chip — no state ever reaches the
 * widget unmapped. The mapping functions carry compile-time never-checks;
 * this test adds the runtime sweep so a state addition that bypasses the
 * switch (e.g. a cast) still fails loudly.
 */

const CHIPS: readonly OrderChip[] = [
  "quoted",
  "approval",
  "placing",
  "confirmed",
  "production",
  "delivered",
  "failed",
];

/** All 16 vcad order states (mirrors the orders.state check constraint in
 *  fabricate/types.ts — typed as OrderState[] so drift fails the build). */
const ORDER_STATES: readonly OrderState[] = [
  "DRAFT",
  "QUOTED",
  "EXPIRED",
  "AUTHORIZED",
  "PENDING_PAYMENT",
  "PAYMENT_FAILED",
  "PAID",
  "SUBMITTED",
  "SUBMIT_FAILED",
  "RECONCILING",
  "IN_PRODUCTION",
  "SHIPPED",
  "DELIVERED",
  "CANCELED",
  "CANCELED_BY_FAB",
  "REFUNDED",
];

describe("order-dock state chips (exhaustive, no unmapped state)", () => {
  it("maps every vcad OrderState to a chip", () => {
    expect(ORDER_STATES).toHaveLength(16);
    for (const state of ORDER_STATES) {
      const chip = orderStateChip(state);
      expect(CHIPS, `${state} maps to a known chip`).toContain(chip);
    }
  });

  it("pins the expected chip per vcad state (the spec's mapping table)", () => {
    expect(orderStateChip("DRAFT")).toBe("quoted");
    expect(orderStateChip("QUOTED")).toBe("quoted");
    expect(orderStateChip("AUTHORIZED")).toBe("approval");
    expect(orderStateChip("PENDING_PAYMENT")).toBe("approval");
    expect(orderStateChip("PAID")).toBe("placing");
    expect(orderStateChip("SUBMITTED")).toBe("placing");
    expect(orderStateChip("RECONCILING")).toBe("placing");
    expect(orderStateChip("IN_PRODUCTION")).toBe("production");
    expect(orderStateChip("SHIPPED")).toBe("production");
    expect(orderStateChip("DELIVERED")).toBe("delivered");
    for (const failed of [
      "EXPIRED",
      "PAYMENT_FAILED",
      "SUBMIT_FAILED",
      "CANCELED",
      "CANCELED_BY_FAB",
      "REFUNDED",
    ] as const) {
      expect(orderStateChip(failed), `${failed} is failed`).toBe("failed");
    }
  });

  it("foregrounds the pending_human overlay: QUOTED + pending authz = approval", () => {
    expect(orderStateChip("QUOTED", "pending_human")).toBe("approval");
    // Any other authz status leaves the order's own chip in charge.
    expect(orderStateChip("QUOTED", "authorized")).toBe("quoted");
    expect(orderStateChip("QUOTED", undefined)).toBe("quoted");
    // The overlay is specific to QUOTED — a terminal state stays terminal.
    expect(orderStateChip("DELIVERED", "pending_human")).toBe("delivered");
  });

  it("maps every one of the 17 kerf JobStates to a chip — no misses", () => {
    expect(KERF_JOB_STATES).toHaveLength(17);
    for (const state of KERF_JOB_STATES) {
      const chip = kerfJobChip(state);
      expect(CHIPS, `${state} maps to a known chip`).toContain(chip);
    }
  });

  it("pins the kerf mapping table (RECONCILING = explained wait, never retry)", () => {
    for (const placing of [
      "QUEUED",
      "SESSION_OPEN",
      "STAGING",
      "STAGED",
      "AUDIT",
      "PLACING",
      "CONFIRMING",
      "RECONCILING",
      "RECONCILED_PLACED",
    ] as const) {
      expect(kerfJobChip(placing), `${placing} is placing`).toBe("placing");
    }
    expect(kerfJobChip("TAKEOVER_WAIT")).toBe("approval");
    expect(kerfJobChip("CONFIRMED")).toBe("confirmed");
    expect(kerfJobChip("TRACKING")).toBe("production");
    expect(kerfJobChip("DELIVERED")).toBe("delivered");
    for (const failed of ["AUDIT_FAILED", "FAILED", "RECONCILED_ABSENT", "CANCELED"] as const) {
      expect(kerfJobChip(failed), `${failed} is failed`).toBe("failed");
    }
  });
});
