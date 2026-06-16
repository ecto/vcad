/** The Receipt — a re-runnable audit ledger of PCB mutations, built by wrapping
 *  each mutation in a deterministic before/after DRC snapshot. Browser- and
 *  node-safe; consumed by both the app (live ledger) and the MCP server. */
export * from "./types.js";
export { buildEntry, buildReceipt, classifyCause, fingerprintSnapshot } from "./engine.js";
export type { MutationStep, BuildReceiptInput } from "./engine.js";
export { ReceiptSession, agentView, headline } from "./session.js";
export type {
  ReceiptSessionDeps,
  ReceiptBoardMeta,
  AgentReceiptView,
  RecordResult,
  ReverifyResult,
} from "./session.js";
export { renderReceiptText, renderReceiptHtml } from "./render.js";
export { snapshotFromViolations } from "./adapter.js";
export { hashHex, HASH_ALGO } from "./hash.js";
