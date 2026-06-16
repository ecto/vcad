/** Bridge a flat DRC violation list (what the in-browser `runDrc` and the
 *  kernel return) into the `DrcSnapshot` the Receipt engine consumes. Computes
 *  the per-rule histogram and error/warning counts the verdict needs. In-app
 *  boards are small, so the full list is always carried (coverage stays "full").
 *  A minimal browser-safe port of the server's `aggregateDrc`. */

import type { DrcSnapshot, DrcViolation } from "./types.js";

export function snapshotFromViolations(violations: DrcViolation[]): DrcSnapshot {
  const byRule: Record<string, number> = {};
  let errors = 0;
  let warnings = 0;
  for (const v of violations) {
    byRule[v.rule] = (byRule[v.rule] ?? 0) + 1;
    if (v.severity === "Error") errors++;
    else if (v.severity === "Warning") warnings++;
  }
  return {
    violations: violations.length,
    errors,
    warnings,
    byRule,
    details: violations,
    sample: violations,
    sampleCapped: false,
  };
}
