/**
 * The Receipt — a re-runnable audit ledger of an agent's PCB work.
 *
 * The PCB mutators (`place_components`, `route_nets`) return only a
 * `document_id` — no diff of what they did (and on some builds they can
 * silently turn a clean board into a shorted one). The Receipt recovers the
 * truth the mutation return hides by WRAPPING each mutation in a deterministic
 * `run_drc` snapshot taken immediately before and after, then diffing them and
 * attributing every violation to a cause (footprint vs routing) so the agent is
 * credited and blamed correctly.
 *
 * Everything here is grounded in fields that `run_drc` actually returns — see
 * `DrcSnapshot`. No tool/kernel changes are required for this v0.
 */

/** One DRC violation as emitted by `run_drc` (the `details`/`sample` entries). */
export interface DrcViolation {
  rule: string;
  severity: string;
  message: string;
  position?: { x: number; y: number };
  actual?: number;
  required?: number;
}

/**
 * A `run_drc` return. `details` is the complete violation array (present when
 * the oracle is called with `detail:"full"`); `sample` is the capped
 * representative subset always present. The engine prefers `details` and
 * degrades to `sample` — flagging when coverage is partial.
 */
export interface DrcSnapshot {
  violations: number;
  errors: number;
  warnings: number;
  byRule: Record<string, number>;
  details?: DrcViolation[];
  sample?: DrcViolation[];
  sampleCapped?: boolean;
}

/** Why a violation exists — derived from the DRC message, not trusted from a field. */
export type Cause =
  | "footprint" // intra-component pad-to-pad — the part's own pin pitch
  | "placement" // pad-to-pad across two components — where they were placed
  | "routing" // trace-to-trace clearance or a copper short — laid by the router
  | "via" // hole-to-hole / drill — a via the router dropped
  | "connectivity" // an unrouted net (UnconnectedNet) or unstitched plane pad (UnstitchedPad)
  | "unknown";

/** A violation's fate across one mutation. */
export type Status = "introduced" | "fixed" | "persisted";

/** Who owns a violation, combining cause + status. */
export type Blame =
  | "credit" // the mutation fixed it (e.g. routed a previously-unconnected net)
  | "blame" // the mutation introduced it (e.g. the router created a short)
  | "pre-existing" // a layout fault the mutation never touched — NOT the agent's fault
  | "carried-over"; // a routing issue that survived this mutation

/** An aggregated bucket of violations sharing (rule, cause, blame, signature). */
export interface ViolationGroup {
  rule: string;
  cause: Cause;
  status: Status;
  blame: Blame;
  count: number;
  /** A representative message + position for the bucket. */
  message: string;
  position?: { x: number; y: number };
  /** Component refdes pair for pad-to-pad faults, when parseable. */
  refs?: [string, string];
}

export type Verdict =
  | "no-op" // nothing changed
  | "clean" // work done, nothing introduced
  | "improved" // net positive: fixed more than (or as much as) it broke
  | "improved-with-regressions" // fixed things but also introduced some
  | "regression"; // introduced errors and fixed nothing

/** One ledger entry: a single agent mutation, wrapped in before/after DRC. */
export interface ReceiptEntry {
  index: number;
  tool: string;
  args: Record<string, unknown>;
  before: { violations: number; errors: number; byRule: Record<string, number> };
  after: { violations: number; errors: number; byRule: Record<string, number> };
  /** after.byRule − before.byRule, per rule — the regression-revealing core. */
  deltaByRule: Record<string, number>;
  /** after.violations − before.violations — the naive headline number. */
  deltaTotal: number;
  introduced: ViolationGroup[];
  fixed: ViolationGroup[];
  persisted: ViolationGroup[];
  tally: {
    credited: number; // fixed (real work done)
    blamed: number; // introduced and the mutation's fault
    preExisting: number; // persisted layout faults, not the agent's fault
    shortsIntroduced: number; // hard net-to-net shorts the mutation created
  };
  regression: boolean;
  verdict: Verdict;
  /** Coverage of the diff: "full" when both snapshots had complete `details`. */
  coverage: "full" | "partial";
  /** Content hash of the canonical `after` snapshot — the deterministic re-run token. */
  fingerprint: string;
}

/** A whole board's audit ledger. */
export interface Receipt {
  board: { title?: string; components?: number; nets?: string[] };
  /** Pre-flight satisfiability note from create_schematic. */
  preflight?: { unconnectedPins?: string[] };
  entries: ReceiptEntry[];
  /** The hash algorithm used for `fingerprint` (e.g. "fnv1a-128"). */
  fingerprintAlgo: string;
  build: { version: string; sha: string };
  /** How the re-run guarantee should be described — honest about its scope. */
  reverification: "deterministic-same-session";
}
