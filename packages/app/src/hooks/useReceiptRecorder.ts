/**
 * Receipt recorder (#280).
 *
 * Watches the focused board and, after each mutation settles, runs DRC and
 * diffs it against the previous settled snapshot to append one attributed
 * Receipt entry — the per-rule delta + credit/blame from @vcad/core's engine.
 * The same kernel DRC the rest of the workspace uses, so the in-app ledger and
 * the server-side `route_nets` receipt attribute identically.
 *
 * Debounced so a burst of edits (e.g. autoroute committing many traces) folds
 * into a single entry. Board/document switches re-seed the baseline and clear
 * the ledger; the very first snapshot seeds without emitting.
 */

import { useEffect, useRef } from "react";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { buildEntry, snapshotFromViolations, type DrcSnapshot, type DrcViolation } from "@vcad/core";
import { runDrc } from "@vcad/engine";
import { useElectronicsStore } from "@/stores/electronics-store";

export function useReceiptRecorder() {
  const active = useElectronicsStore((s) => s.active);
  const document = useDocumentStore((s) => s.document);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;

  const prevRef = useRef<DrcSnapshot | null>(null);
  const boardKeyRef = useRef<string | null>(null);

  useEffect(() => {
    if (!active || !pcb) return;
    const boardKey = String(activeBoardNodeId);
    let cancelled = false;

    const timer = setTimeout(async () => {
      const outcome = await runDrc(pcb);
      if (cancelled) return;
      // An `errored` board was never DRC'd — recording a clean snapshot of it
      // would bake a false-clean into the Receipt. Skip this tick; the next
      // valid eval re-seeds.
      if (outcome.status !== "ok") return;
      const violations = outcome.value as unknown as DrcViolation[];
      const after = snapshotFromViolations(violations);
      const store = useElectronicsStore.getState();

      // Board/document switch (or first sight): re-seed, clear, emit nothing.
      if (boardKeyRef.current !== boardKey) {
        boardKeyRef.current = boardKey;
        prevRef.current = after;
        store.clearReceipt();
        return;
      }

      const before = prevRef.current;
      prevRef.current = after;
      if (!before) return;

      const tag = store.consumePendingMutation();
      const entry = buildEntry(
        { tool: tag?.tool ?? "edit", args: tag?.args ?? {}, before, after },
        store.receiptEntries.length,
      );
      // Only log mutations that actually changed DRC — a no-op move is noise.
      if (entry.verdict !== "no-op") store.appendReceiptEntry(entry);
    }, 350);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [pcb, active, activeBoardNodeId]);
}
