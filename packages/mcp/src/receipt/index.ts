/** The Receipt — a re-runnable audit ledger of an agent's PCB work, built by
 *  wrapping each mutation in a deterministic before/after run_drc snapshot. */
export * from "./types.js";
export { buildEntry, buildReceipt, classifyCause, fingerprintSnapshot } from "./engine.js";
export type { MutationStep, BuildReceiptInput } from "./engine.js";
export { renderReceiptText, renderReceiptHtml } from "./render.js";
export { ReceiptSession, agentView, headline } from "./session.js";
export type {
  ReceiptSessionDeps,
  ReceiptBoardMeta,
  AgentReceiptView,
  RecordResult,
  ReverifyResult,
} from "./session.js";
