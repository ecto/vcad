/**
 * The vendor registry — one package per vendor, data not code.
 *
 * A manifest declares what a vendor can do, over which transport, under
 * which autonomy ceiling. Capability status is measured (by canaries),
 * ceilings are POLICY (set by humans) — a vendor that has declined
 * automation stays pinned at L1 no matter how green its canaries are.
 */

import type { OrderIntent } from "./intent";

/** The autonomy ladder. Rungs are earned per-vendor, never global.
 *  L0: structured handoff, human drives.
 *  L1: kerf stages everything; the human clicks buy in the live view.
 *  L2: kerf clicks buy under a hash-bound mandate + single-use card.
 *  L3: standing mandates for repeat orders. */
export type Autonomy = "L0" | "L1" | "L2" | "L3";

/** How a capability reaches the vendor. `browser` is the universal
 *  last-resort transport; adapters should be designed to be RETIRED into
 *  `api` gracefully when a vendor offers one. */
export type Transport = "browser" | "api" | "email";

export interface Capability {
  transport: Transport;
  /** Registry-relative path to the playbook (browser transport). */
  playbook?: string;
  /** Measured health: green (canary fresh), amber (stale), red (broken),
   *  unproven (never run). Surfaced to clients alongside quotes. */
  status: "green" | "amber" | "red" | "unproven";
  last_verified_at?: string;
}

export interface CanarySpec {
  /** Cron for the scheduled quote-only health run. */
  cron: string;
  playbook: string;
  /** Literal 0: canaries NEVER spend. The type is the enforcement. */
  budget_minor: 0;
}

export interface VendorManifest {
  format: 1;
  id: string;
  label: string;
  /** The job's proxy-level allowlist: the browser session cannot navigate
   *  off these domains. */
  domains: string[];
  region: string;
  intents: Array<OrderIntent["kind"]>;
  /** Process keys for configurator vendors (e.g. "sheet_metal"). */
  processes?: string[];
  capabilities: Partial<
    Record<"quote" | "order" | "track" | "cancel", Capability>
  >;
  autonomy_ceiling: Autonomy;
  policy_notes?: string;
  /** Per-process option space with VENDOR-NATIVE labels — what a
   *  ConfiguratorIntent's `config` keys must conform to. */
  config_schema?: Record<string, unknown>;
  canary?: CanarySpec;
}

/** Structural validation; empty = valid. Enforced in CI. */
export function validateManifest(m: VendorManifest): string[] {
  const problems: string[] = [];
  if (m.format !== 1) problems.push(`unknown manifest format ${m.format}`);
  if (!m.id) problems.push("manifest missing id");
  if (!m.domains?.length)
    problems.push("manifest has no domains — the allowlist would be empty");
  if (!Object.keys(m.capabilities ?? {}).length)
    problems.push("manifest declares no capabilities");
  if (m.canary && m.canary.budget_minor !== 0)
    problems.push("canary budget_minor must be 0 — canaries never spend");
  for (const [name, cap] of Object.entries(m.capabilities ?? {})) {
    if (cap.transport === "browser" && name !== "order" && !cap.playbook) {
      problems.push(
        `capability "${name}" uses browser transport but names no playbook`,
      );
    }
  }
  return problems;
}
