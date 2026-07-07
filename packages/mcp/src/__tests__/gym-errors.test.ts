/**
 * Gym tools must fail-closed: an error return carries `isError: true` so hosts
 * (and the central next_actions enrichment) treat it as a failure rather than
 * reading the `{"error": ...}` body as a successful result.
 */

import { describe, it, expect } from "vitest";
import { gymStep, gymReset, gymObserve, gymClose } from "../tools/gym.js";
import { enrichErrorResult } from "../tools/next-actions.js";

describe("gym error results", () => {
  it("gym_step on an unknown env_id sets isError", () => {
    const out = gymStep({ env_id: "sim_missing", action_type: "torque", values: [] });
    expect(out.isError).toBe(true);
    expect(JSON.parse(out.content[0].text).error).toContain("Unknown env_id");
  });

  it("gym_reset / gym_observe / gym_close on an unknown env_id set isError", () => {
    for (const out of [
      gymReset({ env_id: "sim_missing" }),
      gymObserve({ env_id: "sim_missing" }),
      gymClose({ env_id: "sim_missing" }),
    ]) {
      expect(out.isError).toBe(true);
    }
  });

  it("a gym error result gets next_actions enrichment", () => {
    const out = gymStep({ env_id: "sim_missing", action_type: "torque", values: [] });
    // Because isError is set, the server's central enrichment attaches recovery
    // steps; without isError this would be silently skipped.
    enrichErrorResult(out, "gym_step", { env_id: "sim_missing" });
    const enriched = out as typeof out & {
      structuredContent?: { next_actions?: unknown[] };
    };
    const next = enriched.structuredContent?.next_actions;
    expect(Array.isArray(next)).toBe(true);
    expect(next!.length).toBeGreaterThan(0);
    // The enriched body carries the recovery steps too.
    expect(JSON.parse(out.content[0].text).next_actions).toBeDefined();
  });
});
