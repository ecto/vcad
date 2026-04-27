// Public API for the harness library. The CLI lives in `cli.ts`.

export { loadTask } from "./task.js";
export type { Task, CheckSpec, AntiCheese, Limits, Suite } from "./task.js";
export { getSolver, defaultCubeSolver } from "./solver.js";
export type { Solver, SolverOutput, ToolCall } from "./solver.js";
export { runOne, HARNESS_VERSION } from "./run.js";
export type { RunOptions } from "./run.js";
export {
  buildBlob,
  generateRunId,
  sha256Hex,
  writeBlob,
  BLOB_SCHEMA_VERSION,
} from "./blob.js";
export type { FullRunBlob, PartialGraderBlob } from "./blob.js";
