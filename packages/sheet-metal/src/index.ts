/**
 * @vcad/sheet-metal — Sheet-metal modeling kernel (TypeScript port).
 *
 * Mirrors the Rust `vcad-kernel-sheet` crate exactly. The model is a graph
 * of flat panels connected by cylindrical bends; both the bent 3D body and
 * the flat pattern are derived views, which makes unfold/refold lossless
 * inverses by construction.
 *
 * For the strategic vision and UI plan, see `docs/design/sheet-metal.md`.
 */

export type {
  PanelId,
  BendId,
  Frame,
  Panel,
  BendDirection,
  Bend,
  SheetMetalModel,
} from "./model.js";
export {
  identityFrame,
  frameNormal,
  frameToWorld,
  bendAllowance as panelBendAllowance,
  newModel,
  pushPanel,
  pushBend,
  bfs,
} from "./model.js";

export type { BendTable, BendTableRow, KFactorSource } from "./bend-table.js";
export {
  builtinBendTable,
  lookupKFactor,
  bendAllowance,
  bendDeduction,
  kFactorSourceLabel,
} from "./bend-table.js";

export type { BaseFlangeError } from "./base-flange.js";
export { baseFlangeRect, baseFlangePolygon } from "./base-flange.js";

export type {
  EdgeFlangeError,
  EdgeFlangeParams,
  FlangePosition,
} from "./edge-flange.js";
export { addEdgeFlange } from "./edge-flange.js";

export type {
  FlatPattern,
  FlatCrease,
  TessellatedMesh,
  UnfoldError,
} from "./unfold.js";
export {
  unfold,
  refold,
  flatPatternFromModel,
  tessellate,
} from "./unfold.js";
