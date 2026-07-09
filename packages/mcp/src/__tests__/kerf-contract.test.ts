import { describe, it, expect } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { KERF_JOB_STATES } from "../fabricate/kerf/contract.js";

/**
 * Cross-repo contract drift check: the kerf types mirrored into
 * `src/fabricate/kerf/contract.ts` must match the source of truth in the kerf
 * repo (github.com/ecto/kerf, packages/core/src). Runs only where a kerf
 * clone sits at the conventional sibling-ish path (a dev machine); CI has no
 * clone and skips — the mirror header documents the same discipline.
 */

const KERF_CORE_SRC = "/Users/cam/Developer/kerf/packages/core/src";
const hasKerfClone = existsSync(KERF_CORE_SRC);

const read = (file: string): string => readFileSync(join(KERF_CORE_SRC, file), "utf8");

/** The mirrored literal vocabularies (from contract.ts — keep in sync there). */
const PRICING_BASIS = ["estimate", "quoted", "binding"] as const;
const ORACLE_IDS = [
  "kerf/upload-hash",
  "kerf/quote-extraction",
  "kerf/intent-audit",
  "kerf/confirmation-page",
  "kerf/confirmation-email",
  "kerf/card-settlement",
  "kerf/tracking",
  "kerf/canary",
] as const;
const VERDICTS = ["pass", "fail", "unverifiable"] as const;

describe.skipIf(!hasKerfClone)("kerf contract mirror matches the kerf sources", () => {
  it("all 17 JobState strings appear verbatim in kerf core/job.ts", () => {
    const src = read("job.ts");
    expect(KERF_JOB_STATES).toHaveLength(17);
    for (const state of KERF_JOB_STATES) {
      expect(src, `JobState "${state}" in kerf job.ts`).toContain(`"${state}"`);
    }
    // And nothing extra on kerf's side: every quoted ALL-CAPS literal in the
    // JobState union block is one we mirror.
    const union = src.slice(src.indexOf("export type JobState"), src.indexOf("TRANSITIONS"));
    const literals = [...union.matchAll(/"([A-Z_]+)"/g)].map((m) => m[1]);
    for (const lit of literals) {
      expect(
        (KERF_JOB_STATES as readonly string[]).includes(lit),
        `kerf JobState "${lit}" is mirrored in contract.ts`,
      ).toBe(true);
    }
  });

  it("PricingBasis members appear verbatim in kerf core/quote.ts", () => {
    const src = read("quote.ts");
    for (const basis of PRICING_BASIS) {
      expect(src, `PricingBasis "${basis}"`).toContain(`"${basis}"`);
    }
    expect(src).toContain("export type PricingBasis");
  });

  it("OracleId and Verdict members appear verbatim in kerf core/evidence.ts", () => {
    const src = read("evidence.ts");
    for (const oracle of ORACLE_IDS) {
      expect(src, `OracleId "${oracle}"`).toContain(`"${oracle}"`);
    }
    for (const verdict of VERDICTS) {
      expect(src, `Verdict "${verdict}"`).toContain(`"${verdict}"`);
    }
    expect(src).toContain("export type OracleId");
    expect(src).toContain("export type Verdict");
  });
});
