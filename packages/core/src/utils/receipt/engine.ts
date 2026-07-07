/**
 * Receipt engine — pure functions that turn before/after `run_drc` snapshots
 * into an attributed, fingerprinted ledger entry. No I/O, no kernel, no MCP.
 */

import { hashHex, HASH_ALGO } from "./hash.js";
import type {
  Blame,
  Cause,
  DrcSnapshot,
  DrcViolation,
  ReceiptEntry,
  Status,
  ViolationGroup,
  Verdict,
} from "./types.js";

/** Strip the trailing measurement so the same fault at the same place collapses
 *  to one identity regardless of its magnitude — which drifts as copper moves
 *  but the entities (pads/nets) do not. The measurement is the first signed
 *  decimal-mm token onward (colon-prefixed for clearances, bare for hole-to-hole
 *  / drill / annular). Pad refs like `J1.1` survive — they precede it. */
function messageSignature(message: string): string {
  return message
    .replace(/(:\s*)?-?[\d.]+\s*mm\b.*$/i, "")
    .replace(/\bbelow minimum\b.*$/i, "")
    .trim();
}

/** Classify a violation's cause purely from its DRC message. The kernel emits a
 *  stable format (see crates/vcad-ecad-pcb DRC); we never trust a structured
 *  field here because `netPair` is empty for shorts/hole-to-hole. */
export function classifyCause(v: DrcViolation): { cause: Cause; refs?: [string, string] } {
  const m = v.message;
  if (/^\s*Short\b/i.test(m) || /connected by copper/i.test(m)) return { cause: "routing" };
  if (/Unconnected net/i.test(m)) return { cause: "connectivity" };
  // An unstitched plane pad is a connectivity to-do (drop a stitching via), the
  // same class as an unrouted net — not a footprint/placement defect.
  if (/Unstitched pad/i.test(m)) return { cause: "connectivity" };
  // A same-net bypass is copper the router (or a hand edit) laid over its own
  // net far from any junction — a routing fault, not a footprint/placement one.
  if (/Same-net bypass/i.test(m)) return { cause: "routing" };
  // Pad-to-pad clearance: footprint (same refdes) vs placement (two parts).
  // Checked before the generic `trace` test so a net literally named "trace"
  // can't masquerade a footprint fault as routing.
  const pads = [...m.matchAll(/pad\s+([A-Za-z]+\d+)\.\w+/g)].map((x) => x[1]!);
  if (pads.length >= 2) {
    const refs: [string, string] = [pads[0]!, pads[1]!];
    return { cause: pads[0] === pads[1] ? "footprint" : "placement", refs };
  }
  // Single-pad footprint faults (drill / annular ring) name their component
  // with a trailing ` on <Ref>` — they are the part's own fault, not the router's.
  const onRef = /\bon\s+([A-Za-z]+\d+)\b/.exec(m);
  if (onRef && (/\bdrill\b/i.test(m) || /annular ring/i.test(m))) {
    return { cause: "footprint", refs: [onRef[1]!, onRef[1]!] };
  }
  if (/\btrace\b/i.test(m)) return { cause: "routing" };
  if (/Hole-to-hole/i.test(m)) return { cause: "via" };
  return { cause: "unknown" };
}

const round = (n: number, dp = 2): number => {
  const f = 10 ** dp;
  return Math.round(n * f) / f;
};

/** Multiset identity for diffing: (rule, rounded position, message signature). */
function violationKey(v: DrcViolation): string {
  const px = v.position ? round(v.position.x, 2) : "?";
  const py = v.position ? round(v.position.y, 2) : "?";
  return `${v.rule}|${px}|${py}|${messageSignature(v.message)}`;
}

/** Resolve final blame from cause + status. A fault present both before AND
 *  after a mutation was, by definition, not introduced by it — so anything the
 *  engine cannot pin on this step's routing defaults to "not the agent's". */
function blameOf(cause: Cause, status: Status): Blame {
  if (status === "fixed") return "credit";
  if (status === "introduced") {
    // The router owns anything it newly created — traces, shorts, and the vias it dropped.
    return "blame";
  }
  // persisted: still-unrouted nets and surviving routing faults are carried over;
  // everything else (footprint/placement/via/unknown) pre-dates this mutation.
  if (cause === "connectivity" || cause === "routing") return "carried-over";
  return "pre-existing";
}

interface Bucket {
  rep: DrcViolation;
  cause: Cause;
  refs?: [string, string];
  count: number;
}

function bucketize(violations: DrcViolation[]): Map<string, Bucket> {
  const map = new Map<string, Bucket>();
  for (const v of violations) {
    const key = violationKey(v);
    const ex = map.get(key);
    if (ex) {
      ex.count++;
    } else {
      const { cause, refs } = classifyCause(v);
      map.set(key, { rep: v, cause, refs, count: 1 });
    }
  }
  return map;
}

function toGroup(b: Bucket, status: Status, count: number): ViolationGroup {
  const cause = b.cause;
  return {
    rule: b.rep.rule,
    cause,
    status,
    blame: blameOf(cause, status),
    count,
    message: b.rep.message,
    position: b.rep.position,
    refs: b.refs,
  };
}

/** The complete violation list a snapshot can offer, preferring `details`. */
function violationsOf(s: DrcSnapshot): { list: DrcViolation[]; full: boolean } {
  if (s.details && s.details.length === s.violations) return { list: s.details, full: true };
  if (s.details) return { list: s.details, full: false };
  return { list: s.sample ?? [], full: (s.sample?.length ?? 0) >= s.violations };
}

function deltaByRule(
  before: Record<string, number>,
  after: Record<string, number>,
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const k of new Set([...Object.keys(before), ...Object.keys(after)])) {
    const d = (after[k] ?? 0) - (before[k] ?? 0);
    if (d !== 0) out[k] = d;
  }
  return out;
}

/** Recursively sort object keys so the serialization is order-independent. */
function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(value as Record<string, unknown>).sort()) {
      out[k] = canonicalize((value as Record<string, unknown>)[k]);
    }
    return out;
  }
  return value;
}

/** A content fingerprint of the *board*, not of one `run_drc` invocation. We
 *  hash a normalized projection — counts, per-rule totals, and the violation set
 *  keyed and SORTED — so the token is independent of `detail`/`sample_size` and
 *  of the kernel's violation emission order, and ignores invocation-dependent
 *  fields (`sample`, `byNetPair`, `worstClearance`). Deterministic across re-runs
 *  of the same board on the same build → a verifiable "same board" check. */
export function fingerprintSnapshot(s: DrcSnapshot): string {
  const items = (s.details ?? s.sample ?? [])
    .map((v) => {
      const x = v.position ? round(v.position.x, 4) : "?";
      const y = v.position ? round(v.position.y, 4) : "?";
      return `${v.rule}|${v.severity}|${x}|${y}|${v.message}`;
    })
    .sort();
  const proj = {
    violations: s.violations,
    errors: s.errors,
    warnings: s.warnings,
    byRule: s.byRule,
    items,
  };
  return hashHex(JSON.stringify(canonicalize(proj)));
}

/** The verdict is derived from the per-rule deltas, which come from `byRule` —
 *  always complete even when the per-violation `sample` is capped. A rule count
 *  going up means a violation was introduced; going down means one was fixed. */
function deriveVerdict(delta: Record<string, number>): Verdict {
  const inc = Object.values(delta).some((d) => d > 0);
  const dec = Object.values(delta).some((d) => d < 0);
  if (!inc && !dec) return "no-op";
  if (inc && dec) return "improved-with-regressions";
  if (inc) return "regression";
  return "improved";
}

export interface MutationStep {
  tool: string;
  args: Record<string, unknown>;
  before: DrcSnapshot;
  after: DrcSnapshot;
}

/** Build one ledger entry by diffing the before/after snapshots of a mutation. */
export function buildEntry(step: MutationStep, index: number): ReceiptEntry {
  const beforeV = violationsOf(step.before);
  const afterV = violationsOf(step.after);
  const beforeBuckets = bucketize(beforeV.list);
  const afterBuckets = bucketize(afterV.list);

  const introduced: ViolationGroup[] = [];
  const fixed: ViolationGroup[] = [];
  const persisted: ViolationGroup[] = [];

  const keys = new Set([...beforeBuckets.keys(), ...afterBuckets.keys()]);
  for (const key of keys) {
    const b = beforeBuckets.get(key);
    const a = afterBuckets.get(key);
    const bc = b?.count ?? 0;
    const ac = a?.count ?? 0;
    const persistN = Math.min(bc, ac);
    if (persistN > 0) persisted.push(toGroup((a ?? b)!, "persisted", persistN));
    if (ac > bc) introduced.push(toGroup(a!, "introduced", ac - bc));
    if (bc > ac) fixed.push(toGroup(b!, "fixed", bc - ac));
  }

  const sum = (gs: ViolationGroup[], pred: (g: ViolationGroup) => boolean) =>
    gs.filter(pred).reduce((n, g) => n + g.count, 0);

  const delta = deltaByRule(step.before.byRule, step.after.byRule);
  const deltaTotal = step.after.violations - step.before.violations;

  const credited = fixed.reduce((n, g) => n + g.count, 0);
  const blamed = introduced.reduce((n, g) => n + g.count, 0);
  const preExisting = sum(persisted, (g) => g.blame === "pre-existing");
  // Authoritative from the complete byRule counts — the sample may be capped.
  const shortsIntroduced = Math.max(0, delta.Short ?? 0);

  // Verdict + regression come from the complete byRule aggregates, NEVER from the
  // (possibly capped) per-violation sample — so a partial-coverage diff can never
  // read "clean" on a board that gained shorts.
  const verdict = deriveVerdict(delta);
  const regression = verdict === "regression" || verdict === "improved-with-regressions";

  // Stable ordering: worst (shorts → routing → most numerous) first.
  const order = (g: ViolationGroup) =>
    (/^\s*Short\b/i.test(g.message) ? 0 : g.cause === "routing" ? 1 : 2) * 1e6 - g.count;
  introduced.sort((x, y) => order(x) - order(y));
  fixed.sort((x, y) => y.count - x.count);
  persisted.sort((x, y) => y.count - x.count);

  return {
    index,
    tool: step.tool,
    args: step.args,
    before: {
      violations: step.before.violations,
      errors: step.before.errors,
      byRule: step.before.byRule,
    },
    after: {
      violations: step.after.violations,
      errors: step.after.errors,
      byRule: step.after.byRule,
    },
    deltaByRule: delta,
    deltaTotal,
    introduced,
    fixed,
    persisted,
    tally: { credited, blamed, preExisting, shortsIntroduced },
    regression,
    verdict,
    coverage: beforeV.full && afterV.full ? "full" : "partial",
    fingerprint: fingerprintSnapshot(step.after),
  };
}

export interface BuildReceiptInput {
  board: { title?: string; components?: number; nets?: string[] };
  preflight?: { unconnectedPins?: string[] };
  build: { version: string; sha: string };
  steps: MutationStep[];
}

/** Assemble a full board Receipt from an ordered list of wrapped mutations. */
export function buildReceipt(input: BuildReceiptInput): import("./types.js").Receipt {
  return {
    board: input.board,
    preflight: input.preflight,
    entries: input.steps.map((s, i) => buildEntry(s, i)),
    fingerprintAlgo: HASH_ALGO,
    build: input.build,
    reverification: "deterministic-same-session",
  };
}
