/**
 * ReceiptSession — the bookkeeper.
 *
 * The PCB mutators return only a `document_id`, so an agent that routes a board
 * is told nothing about what it did (or destroyed). A ReceiptSession wraps each
 * mutation: it runs `run_drc` immediately before and after, diffs them through
 * the engine, appends an attributed ledger entry, and hands the agent a compact
 * verdict in place of the blind id. `reverify()` re-runs the oracle and proves
 * the live board still matches what the Receipt claims — the honest,
 * same-session form of "it didn't cheat."
 *
 * The session is decoupled from any transport: inject a `drc` function and pass
 * a `mutate` thunk. It wraps in-process tool calls or MCP calls equally.
 */

import { buildEntry, fingerprintSnapshot } from "./engine.js";
import type { DrcSnapshot, Receipt, ReceiptEntry } from "./types.js";

export interface ReceiptSessionDeps {
  /** Run a (preferably `detail:"full"`) DRC for a document and return the snapshot. */
  drc: (documentId: string) => Promise<DrcSnapshot>;
  build?: { version: string; sha: string };
}

export interface ReceiptBoardMeta {
  title?: string;
  components?: number;
  nets?: string[];
  unconnectedPins?: string[];
}

/** Compact, agent-facing replacement for the silent `{document_id}` return. */
export interface AgentReceiptView {
  document_id: string;
  step: number;
  tool: string;
  verdict: ReceiptEntry["verdict"];
  errors: { before: number; after: number };
  deltaByRule: Record<string, number>;
  credited: number;
  blamed: number;
  shortsIntroduced: number;
  preExisting: number;
  /** "full" when both DRC snapshots had complete details; "partial" otherwise. */
  coverage: "full" | "partial";
  headline: string;
  fingerprint: string;
}

export interface RecordResult<T> {
  entry: ReceiptEntry;
  view: AgentReceiptView;
  result: T;
}

export interface ReverifyResult {
  ok: boolean;
  entry: number;
  stored: string;
  recomputed: string;
}

const plural = (n: number, s: string) => `${n} ${s}${n === 1 ? "" : "s"}`;

/** One-line, agent-readable verdict — the thing the mutator should have said. */
export function headline(e: ReceiptEntry): string {
  const t = e.tally;
  const tail = t.preExisting ? ` (${plural(t.preExisting, "pre-existing fault")} left untouched, not its doing)` : "";
  const partial = e.coverage === "partial" ? " [partial DRC coverage — verdict from rule counts]" : "";
  const base = (() => {
    switch (e.verdict) {
      case "regression":
        return `${e.tool}: REGRESSION — introduced ${plural(t.blamed, "violation")}${
          t.shortsIntroduced ? ` incl. ${plural(t.shortsIntroduced, "hard short")} (board electrically broken)` : ""
        }, fixed ${t.credited}.${tail}`;
      case "improved-with-regressions":
        return `${e.tool}: improved — fixed ${t.credited}, but introduced ${plural(t.blamed, "new violation")}${
          t.shortsIntroduced ? ` incl. ${plural(t.shortsIntroduced, "short")}` : ""
        }.${tail}`;
      case "improved":
        return `${e.tool}: improved — fixed ${plural(t.credited, "violation")}, introduced none.${tail}`;
      case "clean":
        return `${e.tool}: clean — no violations changed.${tail}`;
      default:
        return `${e.tool}: no-op — nothing changed.`;
    }
  })();
  return base + partial;
}

export function agentView(e: ReceiptEntry, documentId: string): AgentReceiptView {
  return {
    document_id: documentId,
    step: e.index + 1,
    tool: e.tool,
    verdict: e.verdict,
    errors: { before: e.before.errors, after: e.after.errors },
    deltaByRule: e.deltaByRule,
    credited: e.tally.credited,
    blamed: e.tally.blamed,
    shortsIntroduced: e.tally.shortsIntroduced,
    preExisting: e.tally.preExisting,
    coverage: e.coverage,
    headline: headline(e),
    fingerprint: e.fingerprint,
  };
}

export class ReceiptSession {
  private entries: ReceiptEntry[] = [];
  /** Set when a mutation ran but its after-DRC failed: the board is mutated and
   *  unaccounted-for, so the next `before` can't be trusted as a clean baseline. */
  private dirty = false;

  constructor(
    private readonly documentId: string,
    private readonly board: ReceiptBoardMeta,
    private readonly deps: ReceiptSessionDeps,
  ) {}

  /** Wrap a mutation: DRC before → run it → DRC after → append an entry.
   *  If the post-mutation DRC fails, the board is left mutated; we mark the
   *  session dirty and rethrow rather than silently fold the lost change into
   *  the next step's baseline. Call `resync()` to accept the gap and continue. */
  async record<T>(
    tool: string,
    args: Record<string, unknown>,
    mutate: () => Promise<T>,
  ): Promise<RecordResult<T>> {
    if (this.dirty) {
      throw new Error(
        "ReceiptSession is unverified after a failed record(); call resync() before continuing",
      );
    }
    const before = await this.deps.drc(this.documentId);
    const result = await mutate();
    let after: DrcSnapshot;
    try {
      after = await this.deps.drc(this.documentId);
    } catch (e) {
      this.dirty = true;
      throw new Error(
        `mutation '${tool}' ran but its after-DRC failed; the board is mutated and unaccounted-for: ${String(e)}`,
      );
    }
    const entry = buildEntry({ tool, args, before, after }, this.entries.length);
    this.entries.push(entry);
    return { entry, view: agentView(entry, this.documentId), result };
  }

  /** Clear the dirty flag after a failed record(), accepting that the gap is
   *  un-audited. The next record()'s `before` becomes a fresh baseline. */
  resync(): void {
    this.dirty = false;
  }

  /** Re-run the oracle now and confirm the live board still matches the most
   *  recent entry's fingerprint. Honest scope: same session/connection only. */
  async reverify(): Promise<ReverifyResult> {
    const last = this.entries[this.entries.length - 1];
    if (!last) throw new Error("nothing recorded yet");
    const recomputed = fingerprintSnapshot(await this.deps.drc(this.documentId));
    return {
      ok: recomputed === last.fingerprint,
      entry: last.index,
      stored: last.fingerprint,
      recomputed,
    };
  }

  receipt(): Receipt {
    return {
      board: { title: this.board.title, components: this.board.components, nets: this.board.nets },
      preflight: this.board.unconnectedPins ? { unconnectedPins: this.board.unconnectedPins } : undefined,
      entries: this.entries,
      fingerprintAlgo: "sha256",
      build: this.deps.build ?? { version: "unknown", sha: "unknown" },
      reverification: "deterministic-same-session",
    };
  }
}
