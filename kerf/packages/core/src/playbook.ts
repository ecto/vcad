/**
 * Playbooks — deterministic vendor drivers as DATA.
 *
 * A playbook is a versioned step graph executed without a model in the
 * loop (Tier 1). The Tier-2 agent is only invoked when a step fails or no
 * playbook exists, and its repair output is a new playbook version.
 *
 * The load-bearing rule: every money-adjacent step carries assertions,
 * and assertions FAIL CLOSED — a failed assertion aborts (or escalates)
 * before the action that would move state at the vendor.
 */

/** Semantic-first element addressing. Resolution order: role+label, text,
 *  css. CSS is the last resort because it is what site redesigns break. */
export interface Selector {
  role?: string;
  label?: string;
  text?: string;
  css?: string;
}

/** Where a fill/select/assert value comes from: a literal, or a
 *  JSON-pointer-ish path into the OrderIntent (e.g. "/config/material",
 *  "/quantity"). Values from the intent, never from the page. */
export type ValueRef =
  | { literal: string | number | boolean }
  | { from_intent: string };

export interface Assertion {
  /** Extraction key (set by a prior `extract` step) to test. */
  subject: string;
  op: "eq" | "approx" | "lte" | "gte" | "matches" | "present";
  value?: ValueRef;
  /** For `approx` on money subjects: allowed delta in minor units
   *  (shipping/tax tolerance). */
  tolerance_minor?: number;
}

export type StepAction =
  | { action: "navigate"; url: string }
  | { action: "upload"; selector: Selector; file: ValueRef }
  | { action: "fill"; selector: Selector; value: ValueRef }
  | { action: "select"; selector: Selector; option_label: ValueRef }
  | { action: "click"; selector: Selector }
  | {
      action: "extract";
      selector: Selector;
      into: string;
      parse?: "money" | "int" | "text" | "date";
    }
  | { action: "screenshot"; into: string }
  /** Suspend the workflow: human resolves in the live view (CAPTCHA, 2FA,
   *  or the L1 buy click), then the run resumes. */
  | { action: "await_hook"; hook: "takeover" | "human_click" };

export interface PlaybookStep {
  id: string;
  do: StepAction;
  /** Marks steps that stage or move vendor-side state (add to cart, apply
   *  config, anything on the checkout path). The validator REJECTS a
   *  money-adjacent step with no assertions. */
  money_adjacent?: boolean;
  assert?: Assertion[];
  /** What happens when the step or its assertions fail.
   *  Default: "abort". Money-adjacent steps may not use "agent_repair" —
   *  the agent repairs navigation, never money state. */
  on_fail?: "abort" | "agent_repair" | "takeover";
}

export interface Playbook {
  format: 1;
  vendor: string;
  capability: "quote" | "order" | "track" | "cancel";
  /** Semver of this playbook, bumped by repair PRs. */
  version: string;
  entry_url: string;
  steps: PlaybookStep[];
  /** Recorded DOM fixtures for offline regression tests. */
  fixtures: string[];
}

/** Structural validation. Returns human-readable problems; empty = valid.
 *  Enforced in CI by scripts/validate-registry.mjs. */
export function validatePlaybook(pb: Playbook): string[] {
  const problems: string[] = [];
  if (pb.format !== 1) problems.push(`unknown playbook format ${pb.format}`);
  if (!pb.steps.length) problems.push("playbook has no steps");
  const seen = new Set<string>();
  for (const step of pb.steps) {
    if (seen.has(step.id)) problems.push(`duplicate step id "${step.id}"`);
    seen.add(step.id);
    if (step.money_adjacent && !(step.assert && step.assert.length)) {
      problems.push(
        `step "${step.id}" is money_adjacent but carries no assertions — assertions fail closed, and money steps must have them`,
      );
    }
    if (step.money_adjacent && step.on_fail === "agent_repair") {
      problems.push(
        `step "${step.id}": agent_repair is not a legal on_fail for a money_adjacent step — the agent repairs navigation, never money state`,
      );
    }
  }
  return problems;
}
