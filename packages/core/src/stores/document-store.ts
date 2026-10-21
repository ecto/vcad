import { create } from "zustand";
import { getKernelWasmSync } from "../wasm-singleton.js";
import { applyCommand, applyToolCall, type Envelope } from "./engine-command.js";
import {
  DocumentEngineClient,
  EngineRefusal,
  sharedEngineTransport,
  type CommandOutcome,
  type HostedEngine,
} from "@vcad/engine";
import type { DocCommand } from "@vcad/ir";
import type { FeatureInput } from "./feature-input.js";
import { mergeSketchConstraints } from "../sketch-constraint-persist.js";
import type {
  Bindings,
  DesignConstraint,
  Document,
  Expr,
  NodeId,
  Parameter,
  Vec3,
  SketchSegment2D,
  SketchConstraint,
  PathCurve,
  Transform3D,
  SweepOp,
  JointKind,
  SceneSettings,
  DrawingSettings,
  Environment,
  Light,
  Background,
  PostProcessing,
  CameraPreset,
  TextAlignment,
  SchematicComponent,
  SchematicWire,
  SchematicLabel,
  SchematicJunction,
  Footprint,
  Pcb,
  BoardOutline,
  EmbroideryDesign,
  FillParams,
  AnalysisStudy,
} from "@vcad/ir";
import { DEFAULT_FILL_PARAMS } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import type {
  PartInfo,
  PrimitiveKind,
  BooleanType,
  ExtrudePartInfo,
  RevolvePartInfo,
  SweepPartInfo,
  LoftPartInfo,
  ImportedMeshPartInfo,
  EmbroideryPatternPartInfo,
  PcbBoardPartInfo,
  SketchPlane,
} from "../types.js";
import {
  isExtrudePart,
  isRevolvePart,
  isSweepPart,
  isLoftPart,
  isTextPart,
  isStitchEligible,
  getSketchPlaneDirections,
} from "../types.js";
import { useUiStore } from "./ui-store.js";
import { useParametersStore } from "./parameters-store.js";

// ---------------------------------------------------------------------------
// CRDT bridge types
// ---------------------------------------------------------------------------

/** Result from legacy WasmDocumentEngine mutation methods */
interface CrdtMutationResult {
  document: Document;
  parts: PartInfo[];
  createdFeatureId?: string;
}

/** Result from typed WasmDocumentEngine API methods */
interface ApiResult {
  document: Document;
  parts: PartInfo[];
  consumedPartIds: string[];
  createdFeatureId?: string;
}

/** Minimal interface for WasmDocumentEngine (matches WASM exports) */
export interface WasmDocumentEngine {
  /** Apply a JSON `DocCommand`; returns the result envelope. Call through
   *  `applyCommand` (engine-command.ts). */
  apply(command_json: string): string;
  /** Lower + apply an AI tool call; see `applyToolCall`. */
  apply_tool(tool: string, args_json: string): string;
  // Typed API — returns ApiResult with consumedPartIds
  add_feature(input_json: string): ApiResult;
  update_feature(stable_id: string, input_json: string): ApiResult;
  delete_feature_by_id(stable_id: string): ApiResult;
  set_translation(stable_id: string, x: number, y: number, z: number): ApiResult;
  set_rotation(stable_id: string, x: number, y: number, z: number): ApiResult;
  set_scale(stable_id: string, x: number, y: number, z: number): ApiResult;
  set_material(stable_id: string, material: string): ApiResult;
  set_visible(stable_id: string, visible: boolean): ApiResult;
  rename_feature(stable_id: string, name: string): ApiResult;
  set_joint_state(stable_id: string, state: number): ApiResult;

  /**
   * Rewrite v1 parameter-binding keys (`"<nodeId>:<fieldPath>"`) onto this
   * engine's node ids. Migration renumbers every node, so bindings loaded
   * from a legacy/loon file are dangling until remapped. A no-op for
   * CRDT-native loads. Optional so an older wasm build still loads.
   */
  remapBindings?(bindings_json: string): {
    bindings: Record<string, string>;
    dropped: string[];
  };

  // Legacy low-level CRDT methods (for electronics, scene settings, param updates)
  create_feature(kind: string, params_json: string): CrdtMutationResult;
  delete_feature(feature_id_json: string): CrdtMutationResult;
  set_param(
    feature_id_json: string,
    key: string,
    value_json: string,
  ): CrdtMutationResult;
  move_feature(
    feature_id_json: string,
    position_json: string,
  ): CrdtMutationResult;
  undo(): CrdtMutationResult;
  redo(): CrdtMutationResult;
  can_undo(): boolean;
  can_redo(): boolean;
  save(): Uint8Array;
  free(): void;
  get_ordered_features_json(): string;
  get_document_json(): string;
  get_parts_json(): string;

  // Sync API (used by the collab transport layer)
  merge_remote(ops_json: string): CrdtMutationResult;
  get_sync_clock(): string;
  get_ops_since(remote_clock_json: string): string;
  compute_position_between(before_id_json: string, after_id_json: string): string;
  import_ir(ir_json: string): CrdtMutationResult;
}

/** Constructor for WasmDocumentEngine */
export interface WasmDocumentEngineConstructor {
  new (): WasmDocumentEngine;
  load(bytes: Uint8Array): WasmDocumentEngine;
  from_v1_json(json: string): WasmDocumentEngine;
}

/**
 * CRDT value type — mirrors Rust vcad_crdt::Value.
 * Used by `setFeatureParam` and legacy methods that still call `set_param`.
 */
type CrdtValue =
  | { F64: number }
  | { Vec3: [number, number, number] }
  | { Bool: boolean }
  | { String: string }
  | { FeatureRef: string }
  | { FeatureRefList: string[] }
  | { Sketch: string };

// Legacy CRDT value helpers — used by methods still calling set_param/create_feature
function crdtF64(v: number): CrdtValue {
  return { F64: v };
}
function crdtVec3(v: Vec3): CrdtValue {
  return { Vec3: [v.x, v.y, v.z] };
}
function crdtBool(v: boolean): CrdtValue {
  return { Bool: v };
}
function crdtStr(v: string): CrdtValue {
  return { String: v };
}
function crdtRef(v: string): CrdtValue {
  return { FeatureRef: v };
}

/**
 * Re-export the canonical `VcadFile` from `save-load` so existing imports of
 * `VcadFile` from `@vcad/core` keep working. The old flat shape that used to
 * live here is now the `VcadFileLegacy` variant of the tagged union.
 */
export type { VcadFile } from "../utils/save-load.js";
import type { VcadFile } from "../utils/save-load.js";

export interface PcbCreateOptions {
  width?: number;        // mm, default 50
  height?: number;       // mm, default 30
  layers?: number;   // copper count: 1 or even 2..MAX_COPPER_LAYERS (default 2)
  thickness?: number;    // mm, default 1.6
  traceWidth?: number;   // mm, default 0.15
  clearance?: number;    // mm, default 0.15
  name?: string;         // default "PCB Board"
}

export interface DocumentState {
  document: Document;
  parts: PartInfo[];
  partIndex: Map<string, PartInfo>; // O(1) lookup by part id
  consumedParts: Record<string, PartInfo>; // Parts consumed by booleans, keyed by id
  nextNodeId: number;
  isDirty: boolean;

  // Document persistence metadata
  documentId: string | null;
  documentName: string;
  lastSavedAt: number | null;

  /** Whether a parametric drag is in progress (enables LOD mode) */
  isParameterDragging: boolean;

  /** Whether a transient batch of mutations is in progress (e.g. an AI is
   *  streaming a sequence of tool calls). When true, evals skip clash
   *  detection so the viewport paints in ~30ms instead of waiting for the
   *  O(n²) clash loop. A refinement pass with full clash runs ~100ms after
   *  this flips back to false. */
  isTransientEval: boolean;

  /** Loon source code — non-null when document was loaded from loon format. */
  loonSource: string | null;

  // --------------- CRDT engine ---------------
  /** The CRDT engine instance, or null if not yet initialized. */
  _crdtEngine: WasmDocumentEngine | null;
  /** The CRDT engine constructor (stored for creating new engines). */
  _crdtEngineClass: WasmDocumentEngineConstructor | null;
  /**
   * Async command client over `_crdtEngine` (store-as-view). Actions moved
   * to it return promises and fold engine deltas instead of re-reading the
   * whole document; the rest still call the engine synchronously, and the
   * client's state-hash check resyncs its view after those.
   */
  _engineClient: DocumentEngineClient;
  /** Apply `command` through the client and fold the result into the store.
   *  Resolves to the created feature id, if any; rejects on refusal. */
  dispatch: (command: DocCommand) => Promise<string | undefined>;
  /** Initialize the CRDT engine. Called once after WASM loads. */
  _initCrdt: (EngineClass: WasmDocumentEngineConstructor) => void;
  /** Save CRDT document to bytes (returns null if engine not initialized). */
  saveCrdt: () => Uint8Array | null;
  /** Load CRDT document from bytes. */
  loadCrdt: (
    bytes: Uint8Array,
    EngineClass: WasmDocumentEngineConstructor,
  ) => void;

  // ─── Collab sync API ───────────────────────────────────────────────────
  /** Merge remote CRDT ops (JSON array). Returns true if state changed. */
  mergeRemoteOps: (opsJson: string) => boolean;
  /** Get the local sync clock as JSON (for delta sync). */
  getSyncClock: () => string;
  /** Get ops the remote hasn't seen (JSON). */
  getOpsSince: (remoteClockJson: string) => string;

  // mutations
  addPrimitive: (kind: PrimitiveKind) => string;
  /**
   * Insert a stdlib or user-published part instance.
   *
   * @param path    Part source path, e.g. `"std:fastener.bolt.socket-head"`.
   * @param version Pinned version string, e.g. `"1.0"`.
   * @param params  Parameter name → value map.
   * @returns       The created part id, or empty string on failure.
   */
  addPartInstance: (
    path: string,
    version: string,
    params: Record<string, unknown>,
  ) => string;
  removePart: (partId: string) => Promise<void>;
  setTranslation: (partId: string, offset: Vec3, skipUndo?: boolean) => void;
  setRotation: (partId: string, angles: Vec3, skipUndo?: boolean) => void;
  setScale: (partId: string, factor: Vec3, skipUndo?: boolean) => void;
  updatePrimitiveOp: (partId: string, op: unknown, skipUndo?: boolean) => void;
  updateSweepOp: (
    partId: string,
    updates: Partial<SweepOp>,
    skipUndo?: boolean,
  ) => void;
  /** Set a single CRDT param on a feature by its part ID. */
  setFeatureParam: (
    partId: string,
    key: string,
    value: CrdtValue,
    skipUndo?: boolean,
  ) => void;
  renamePart: (partId: string, name: string) => Promise<void>;
  updateBooleanType: (
    partId: string,
    newType: BooleanType,
    skipUndo?: boolean,
  ) => Promise<void>;
  applyBoolean: (
    type: BooleanType,
    partIdA: string,
    partIdB: string,
  ) => string | null;
  duplicateParts: (partIds: string[]) => string[];
  loadDocument: (file: VcadFile) => void;
  /** Merge generated IR document into current document (for AI generation) */
  addFromIR: (generatedDoc: Document, name?: string) => string | null;
  addExtrude: (
    plane: SketchPlane,
    origin: Vec3,
    segments: SketchSegment2D[],
    direction: Vec3,
    options?: {
      twist_angle?: number;
      scale_end?: number;
    },
  ) => string | null;
  addRevolve: (
    plane: SketchPlane,
    origin: Vec3,
    segments: SketchSegment2D[],
    axisOrigin: Vec3,
    axisDir: Vec3,
    angleDeg: number,
  ) => string | null;
  addSweep: (
    plane: SketchPlane,
    origin: Vec3,
    segments: SketchSegment2D[],
    path: PathCurve,
    options?: {
      twist_angle?: number;
      scale_start?: number;
      scale_end?: number;
    },
  ) => string | null;
  addLoft: (
    profiles: Array<{
      plane: SketchPlane;
      origin: Vec3;
      segments: SketchSegment2D[];
    }>,
    options?: { closed?: boolean },
  ) => string | null;
  setPartMaterial: (partId: string, materialKey: string) => Promise<void>;
  /** No-op — CRDT undo handles granularity automatically. */
  pushUndoSnapshot: () => void;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  /** Whether the engine has undo history available. */
  canUndo: () => boolean;
  /** Whether the engine has redo history available. */
  canRedo: () => boolean;
  markSaved: () => void;
  setDocumentMeta: (id: string, name: string) => void;
  setDocumentName: (name: string) => void;
  newDocument: (id: string, name: string) => void;
  // Assembly operations
  setInstanceTransform: (
    instanceId: string,
    transform: Transform3D,
    skipUndo?: boolean,
  ) => void;
  setInstanceMaterial: (instanceId: string, materialKey: string) => Promise<void>;
  setJointState: (jointId: string, state: number, skipUndo?: boolean) => void;
  createPartDef: (partId: string, name?: string) => string | null;
  createInstance: (
    partDefId: string,
    name?: string,
    transform?: Transform3D,
  ) => string;
  addJoint: (config: {
    parentInstanceId: string | null;
    childInstanceId: string;
    parentAnchor: Vec3;
    childAnchor: Vec3;
    kind: JointKind;
    name?: string;
  }) => string;
  deleteInstance: (instanceId: string) => void;
  deleteJoint: (jointId: string) => Promise<void>;
  setGroundInstance: (instanceId: string) => void;
  renameInstance: (instanceId: string, name: string) => Promise<void>;
  addImportedMesh: (
    positions: Float32Array,
    indices: Uint32Array,
    normals?: Float32Array,
    source?: string,
  ) => string;
  addEmbroideryPattern: (design: EmbroideryDesign, source?: string) => string;
  addTextEmbroidery: (options: {
    text: string;
    height: number;
    color?: [number, number, number];
    stitchType?: "running" | "satin" | "fill";
    stitchLength?: number;
    density?: number;
    satinWidth?: number;
    fillAngle?: number;
    letterSpacing?: number;
    lineSpacing?: number;
    alignment?: "left" | "center" | "right";
  }) => Promise<{ partId: string; result: Record<string, unknown> } | null>;
  // Embroidery editing mutations
  setThreadColor: (nodeId: NodeId, threadIdx: number, color: [number, number, number]) => Promise<void>;
  setThreadName: (nodeId: NodeId, threadIdx: number, name: string) => Promise<void>;
  setStitchGroupFillParams: (nodeId: NodeId, groupIdx: number, params: Partial<FillParams>) => void;
  optimizeJumpStitches: (nodeId: NodeId) => void;
  // Modify operations (wrap existing part)
  addFillet: (partId: string, radius: number) => string | null;
  addChamfer: (partId: string, distance: number) => string | null;
  addShell: (partId: string, thickness: number) => string | null;
  addLinearPattern: (
    partId: string,
    direction: Vec3,
    count: number,
    spacing: number,
  ) => string | null;
  addCircularPattern: (
    partId: string,
    axisOrigin: Vec3,
    axisDir: Vec3,
    count: number,
    angleDeg: number,
  ) => string | null;
  addMirror: (partId: string, plane: "XY" | "XZ" | "YZ") => string | null;
  addStitch: (partId: string, options: {
    stitchType?: "running" | "satin" | "fill";
    color?: [number, number, number];
    stitchLength?: number;
    density?: number;
    satinWidth?: number;
    fillAngle?: number;
  }) => Promise<string | null>;
  addText: (options: {
    text: string;
    height: number;
    depth: number;
    alignment?: TextAlignment;
    letterSpacing?: number;
    lineSpacing?: number;
  }) => string | null;
  // Incremental evaluation actions (no-ops — CRDT replaces document wholesale)
  clearDirtyNodes: () => Set<NodeId>;
  setParameterDragging: (dragging: boolean) => void;
  setTransientEval: (active: boolean) => void;
  // Visibility toggle
  setPartVisible: (partId: string, visible: boolean) => Promise<void>;
  /** Run an AI tool call through the Rust lowering (`vcad_app::tools`).
   *  `null` when the kernel predates it; see `applyToolCall`. */
  applyTool: (tool: string, args: unknown) => Envelope | null;
  // Reorder parts in tree
  reorderPart: (partId: string, newIndex: number) => void;
  // Analyze mode (#592): persist solver studies on the document (CRDT
  // singleton feature, like scene settings)
  setAnalysisStudies: (studies: AnalysisStudy[]) => void;
  // Scene settings actions
  setSceneSettings: (settings: SceneSettings) => void;
  /** Persist drawing sheet settings (title block, sections, BOM) on the document. */
  setDrawingSettings: (settings: DrawingSettings) => void;
  updateEnvironment: (environment: Environment) => void;
  updateLights: (lights: Light[]) => void;
  addLight: (light: Light) => void;
  removeLight: (lightId: string) => void;
  updateLight: (lightId: string, updates: Partial<Light>) => void;
  updateBackground: (background: Background) => void;
  updatePostProcessing: (postProcessing: PostProcessing) => void;
  addCameraPreset: (preset: CameraPreset) => void;
  removeCameraPreset: (presetId: string) => void;

  // Electronics (ECAD) mutations
  initSchematic: (title?: string) => void;
  initPcb: (options?: PcbCreateOptions) => NodeId;
  importPcb: (pcb: Pcb, name?: string) => NodeId;
  syncSchematicToPcb: (
    boardNodeId: NodeId,
    netlist?: {
      nets: { name: string; connections: { component_ref: string; pin_number: string }[] }[];
    },
    opts?: { placeUnplaced?: boolean },
  ) => void;
  moveSchematicComponent: (idx: number, position: Vec3) => void;
  moveSchematicComponentWithWires: (idx: number, position: Vec3, wireUpdates: { wireIdx: number; endpoint: "start" | "end"; pos: { x: number; y: number } }[]) => void;
  moveFootprint: (nodeId: NodeId, idx: number, position: Vec3) => void;
  rotateFootprint: (nodeId: NodeId, idx: number, angleDeg: number) => void;
  flipFootprint: (nodeId: NodeId, idx: number) => void;
  addTrace: (nodeId: NodeId, trace: {
    start: Vec3;
    end: Vec3;
    width: number;
    layer: string;
    net: string;
  }) => void;
  removeTrace: (nodeId: NodeId, idx: number) => void;
  addVia: (nodeId: NodeId, via: {
    position: Vec3;
    diameter: number;
    drill: number;
    startLayer: string;
    endLayer: string;
    net: string;
  }) => void;
  removeVia: (nodeId: NodeId, idx: number) => void;

  // Schematic editing mutations
  addSchematicComponent: (comp: SchematicComponent, boardNodeId?: NodeId) => void;
  removeSchematicComponent: (idx: number, boardNodeId?: NodeId) => void;
  updateSchematicComponent: (idx: number, updates: Partial<SchematicComponent>, boardNodeId?: NodeId) => void;
  addSchematicWire: (wire: SchematicWire) => void;
  removeSchematicWire: (idx: number) => void;
  addSchematicLabel: (label: SchematicLabel) => void;
  removeSchematicLabel: (idx: number) => void;
  addSchematicJunction: (junction: SchematicJunction) => void;

  // PCB editing mutations
  addFootprint: (nodeId: NodeId, fp: Footprint) => void;
  removeFootprint: (nodeId: NodeId, idx: number) => void;
  /** Replace the board outline (vertices, cutouts, thickness). Re-extrudes the slab. */
  setBoardOutline: (outline: BoardOutline) => void;
  /** Resize the board to a W×H rectangle (origin corner at [0,0]), preserving thickness + cutouts. */
  resizeBoard: (width: number, height: number) => void;

  // Design constraints (document-level geometric solver)
  /** Persist a committed sketch's session constraints onto the document,
   *  anchored at its sketch node (replaces prior constraints for that node). */
  persistSketchConstraints: (partId: string, session: SketchConstraint[]) => void;
  /** Append a design constraint (id auto-assigned) and re-solve. */
  addDesignConstraint: (constraint: Omit<DesignConstraint, "id">) => DesignConstraintSolveReport | null;
  /** Remove a design constraint by id. Geometry stays where it is. */
  removeDesignConstraint: (id: string) => void;
  /** Patch a dimensional constraint's value / driven flag, then re-solve. */
  updateDesignConstraint: (
    id: string,
    patch: { value?: number | string; driven?: boolean },
  ) => DesignConstraintSolveReport | null;
  /** Re-solve the document's design constraints via the kernel WASM solver.
   *  `extraFixed` pins footprints for this solve only (drag anchoring). */
  solveDesignConstraints: (opts?: {
    extraFixed?: Array<{ node: number; ref: string }>;
  }) => DesignConstraintSolveReport | null;
}

/** Subset of the kernel's design-constraint solve report the app consumes. */
export interface DesignConstraintSolveReport {
  converged: boolean;
  groups: Array<{ node: number; status: string; converged: boolean; dof: number }>;
  movedFootprints: string[];
  movedVertices: string[];
  drivenValues: Array<{ id: string; value: number }>;
  residuals: Array<{ id: string; residual: number; driven: boolean }>;
  errors: string[];
  warnings: string[];
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/** Build a Map index from parts array for O(1) lookups */
function buildPartIndex(parts: PartInfo[]): Map<string, PartInfo> {
  const index = new Map<string, PartInfo>();
  for (const part of parts) {
    index.set(part.id, part);
  }
  return index;
}

/** Get a PcbBoard node's Pcb data by node ID.
 *  Falls back to `doc.pcb` when the materializer stores PCB data there
 *  instead of inlining it in the node op (CRDT engine emits Empty nodes). */
export function getNodePcb(doc: Document, nodeId: NodeId): Pcb | null {
  const node = doc.nodes[String(nodeId)];
  if (node?.op.type === "PcbBoard") return (node.op as { type: "PcbBoard"; board: Pcb }).board;
  // CRDT materializer stores PCB data in doc.pcb and creates an Empty node
  if (doc.pcb) return doc.pcb;
  return null;
}

/** A PCB board's world placement, read from its transform wrapper nodes. */
export interface PcbBoardTransform {
  /** Translation offset (kernel mm, Z-up). */
  position: Vec3;
  /** Euler angles in DEGREES, X→Y→Z order — matches the kernel evaluator. */
  rotationDeg: Vec3;
  /** Per-axis scale factor. */
  scale: Vec3;
}

/**
 * The world transform of a PcbBoard part, read from its scale/rotate/translate
 * wrapper nodes. Mirrors the kernel evaluator's compose order
 * (translate ∘ rotate ∘ scale, baked into the board solid), so PcbScene's
 * copper / interaction plane and the camera framing can track a board that has
 * been moved or rotated as a part. Falls back to identity for any missing or
 * non-transform node.
 */
export function getPcbBoardTransform(
  doc: Document,
  part: PcbBoardPartInfo,
): PcbBoardTransform {
  const op = (id: NodeId) => doc.nodes[String(id)]?.op;
  const t = op(part.translateNodeId);
  const r = op(part.rotateNodeId);
  const s = op(part.scaleNodeId);
  return {
    position: t?.type === "Translate" ? t.offset : { x: 0, y: 0, z: 0 },
    rotationDeg: r?.type === "Rotate" ? r.angles : { x: 0, y: 0, z: 0 },
    scale: s?.type === "Scale" ? s.factor : { x: 1, y: 1, z: 1 },
  };
}

/** Find the PcbBoard part whose board node matches `boardNodeId`, if any. */
export function findPcbBoardPart(
  parts: PartInfo[],
  boardNodeId: NodeId,
): PcbBoardPartInfo | null {
  for (const p of parts) {
    if (p.kind === "pcb-board" && p.boardNodeId === boardNodeId) {
      return p as PcbBoardPartInfo;
    }
  }
  return null;
}

/** Get an EmbroideryPattern node's design data by node ID. */
export function getNodeEmbroideryDesign(doc: Document, nodeId: NodeId): EmbroideryDesign | null {
  const node = doc.nodes[String(nodeId)];
  if (node?.op.type === "EmbroideryPattern") return (node.op as { type: "EmbroideryPattern"; design: EmbroideryDesign }).design;
  return null;
}

/** Find all PcbBoard node IDs in the document. */
export function getPcbNodeIds(doc: Document): NodeId[] {
  const ids: NodeId[] = [];
  for (const [, node] of Object.entries(doc.nodes)) {
    if (node.op.type === "PcbBoard") ids.push(node.id);
  }
  return ids;
}

/** Encode sketch segments + plane into a JSON string for the typed API. */
function sketchJson(segments: SketchSegment2D[], plane: SketchPlane, origin: Vec3): string {
  const { x_dir, y_dir } = getSketchPlaneDirections(plane);
  return JSON.stringify({ type: "Sketch2D", origin, x_dir, y_dir, segments });
}

// ---------------------------------------------------------------------------
// Result helpers
// ---------------------------------------------------------------------------

/** Compute max node ID from a document for nextNodeId tracking. */
function computeNextNodeId(doc: Document): number {
  let maxNodeId = 0;
  for (const nodeIdStr of Object.keys(doc.nodes)) {
    const nid = Number(nodeIdStr);
    if (nid > maxNodeId) maxNodeId = nid;
  }
  return maxNodeId + 1;
}

/**
 * CRDT engine methods that mutate persistent document state.
 * Proxy-intercepted in read-only share sessions to prevent viewer edits.
 */
const MUTATION_METHODS = new Set<string>([
  "add_feature",
  "update_feature",
  "delete_feature_by_id",
  "set_translation",
  "set_rotation",
  "set_scale",
  "set_material",
  "set_visible",
  "rename_feature",
  "set_joint_state",
  "create_feature",
  "delete_feature",
  "set_param",
  "move_feature",
  "import_ir",
  "undo",
  "redo",
  // DocCommand entry points: return a JSON envelope string, not an object.
  "apply",
  "apply_tool",
]);

/** Mutation methods whose result is a `DocCommand` envelope string. */
const ENVELOPE_METHODS = new Set<string>(["apply", "apply_tool"]);

/**
 * Build a no-op result object shaped like ApiResult/CrdtMutationResult that
 * leaves the store state unchanged. Used when a mutation call is blocked by
 * the read-only share guard.
 */
function makeNoOpResult(target: WasmDocumentEngine): ApiResult & CrdtMutationResult {
  const document: Document = JSON.parse(target.get_document_json());
  const parts: PartInfo[] = JSON.parse(target.get_parts_json());
  return {
    document,
    parts,
    consumedPartIds: [],
  };
}

/**
 * Wrap a WasmDocumentEngine so that any call to a persistent-state mutation
 * method is intercepted when the UI is in a read-only share session. Blocked
 * calls dispatch a `vcad:fork-prompt` event (picked up by App.tsx to open the
 * sign-in-to-fork modal) and return a no-op result that keeps store state
 * unchanged.
 */
/** @internal Exported for tests. */
export function wrapEngineWithReadOnlyGuard(
  engine: WasmDocumentEngine,
): WasmDocumentEngine {
  return new Proxy(engine, {
    get(target, prop, receiver) {
      const value = Reflect.get(target, prop, receiver);
      if (typeof value !== "function") return value;
      const name = prop as string;
      if (!MUTATION_METHODS.has(name)) {
        return (value as (...a: unknown[]) => unknown).bind(target);
      }
      return function (this: unknown, ...args: unknown[]) {
        const readOnly = useUiStore.getState().readOnlyShare;
        if (readOnly) {
          if (typeof window !== "undefined") {
            window.dispatchEvent(
              new CustomEvent("vcad:fork-prompt", { detail: readOnly }),
            );
          }
          const noOp = makeNoOpResult(target);
          return ENVELOPE_METHODS.has(name)
            ? JSON.stringify({ ok: true, result: noOp })
            : noOp;
        }
        return (value as (...a: unknown[]) => unknown).apply(target, args);
      };
    },
  });
}

/**
 * Apply a typed API result (has consumedPartIds) to the store.
 */
function applyApiResult(result: ApiResult): Partial<DocumentState> {
  const partIndex = buildPartIndex(result.parts);
  const consumedParts: Record<string, PartInfo> = {};
  for (const id of result.consumedPartIds) {
    const part = partIndex.get(id);
    if (part) consumedParts[id] = part;
  }
  return {
    document: result.document,
    parts: result.parts,
    partIndex,
    consumedParts,
    nextNodeId: computeNextNodeId(result.document),
    isDirty: true,
  };
}

/**
 * Apply a legacy CRDT mutation result (no consumedPartIds) to the store.
 * Computes consumed parts by scanning sourcePartIds/sourcePartId references.
 */
function applyLegacyResult(result: CrdtMutationResult): Partial<DocumentState> {
  const partIndex = buildPartIndex(result.parts);
  const consumedParts: Record<string, PartInfo> = {};
  for (const part of result.parts) {
    if ("sourcePartIds" in part && Array.isArray(part.sourcePartIds)) {
      for (const refId of part.sourcePartIds as string[]) {
        const consumed = partIndex.get(refId);
        if (consumed) consumedParts[refId] = consumed;
      }
    }
    if ("sourcePartId" in part && typeof part.sourcePartId === "string") {
      const consumed = partIndex.get(part.sourcePartId as string);
      if (consumed) consumedParts[part.sourcePartId as string] = consumed;
    }
  }
  return {
    document: result.document,
    parts: result.parts,
    partIndex,
    consumedParts,
    nextNodeId: computeNextNodeId(result.document),
    isDirty: true,
  };
}

// ---------------------------------------------------------------------------
// Module-level helpers
// ---------------------------------------------------------------------------

/** Apply an undo/redo through the store's client; a refusal for lack of
 *  history is swallowed (the button raced the last step). */
async function historyStep(get: () => DocumentState, type: "undo" | "redo"): Promise<void> {
  try {
    await get().dispatch({ type });
  } catch (e) {
    if (e instanceof EngineRefusal && e.error.kind === "nothing_to") return;
    throw e;
  }
}

/** Cached singleton feature IDs (lazily created, cleared on engine init). */
let _sceneSettingsFeatureId: string | null = null;
let _schematicFeatureId: string | null = null;
let _analysisStudiesFeatureId: string | null = null;

function getOrCreateAnalysisStudiesFeature(state: DocumentState): string {
  const engine = state._crdtEngine!;
  if (_analysisStudiesFeatureId) return _analysisStudiesFeatureId;

  const featuresJson = engine.get_ordered_features_json();
  const features: { id: string; kind: string }[] = JSON.parse(featuresJson);
  const existing = features.find((f) => f.kind === "analysis-studies");
  if (existing) {
    _analysisStudiesFeatureId = existing.id;
    return existing.id;
  }

  const result = applyCommand(engine, { type: "create_feature", kind: "analysis-studies" });
  if (result.createdFeatureId) {
    _analysisStudiesFeatureId = result.createdFeatureId;
    return result.createdFeatureId;
  }
  return "";
}

function getOrCreateSceneFeature(state: DocumentState): string {
  const engine = state._crdtEngine!;
  if (_sceneSettingsFeatureId) return _sceneSettingsFeatureId;

  const featuresJson = engine.get_ordered_features_json();
  const features: { id: string; kind: string }[] = JSON.parse(featuresJson);
  const existing = features.find((f) => f.kind === "scene-settings");
  if (existing) {
    _sceneSettingsFeatureId = existing.id;
    return existing.id;
  }

  const result = applyCommand(engine, { type: "create_feature", kind: "scene-settings" });
  if (result.createdFeatureId) {
    _sceneSettingsFeatureId = result.createdFeatureId;
    return result.createdFeatureId;
  }
  return "";
}

let _drawingSettingsFeatureId: string | null = null;

function getOrCreateDrawingFeature(state: DocumentState): string {
  const engine = state._crdtEngine!;
  if (_drawingSettingsFeatureId) return _drawingSettingsFeatureId;

  const featuresJson = engine.get_ordered_features_json();
  const features: { id: string; kind: string }[] = JSON.parse(featuresJson);
  const existing = features.find((f) => f.kind === "drawing-settings");
  if (existing) {
    _drawingSettingsFeatureId = existing.id;
    return existing.id;
  }

  const result = applyCommand(engine, { type: "create_feature", kind: "drawing-settings" });
  if (result.createdFeatureId) {
    _drawingSettingsFeatureId = result.createdFeatureId;
    return result.createdFeatureId;
  }
  return "";
}


/** The singleton design-constraints CRDT feature (created on demand). */
function getOrCreateSchematicFeature(state: DocumentState): string {
  const engine = state._crdtEngine!;
  if (_schematicFeatureId) return _schematicFeatureId;

  const featuresJson = engine.get_ordered_features_json();
  const features: { id: string; kind: string }[] = JSON.parse(featuresJson);
  const existing = features.find((f) => f.kind === "schematic");
  if (existing) {
    _schematicFeatureId = existing.id;
    return existing.id;
  }

  const result = applyCommand(engine, { type: "create_feature", kind: "schematic" });
  if (result.createdFeatureId) {
    _schematicFeatureId = result.createdFeatureId;
    return result.createdFeatureId;
  }
  return "";
}

function getPcbBoardFeatureId(state: DocumentState): string {
  const engine = state._crdtEngine!;
  const featuresJson = engine.get_ordered_features_json();
  const features: { id: string; kind: string }[] = JSON.parse(featuresJson);
  const pcb = features.find((f) => f.kind === "pcb-board");
  return pcb?.id ?? "";
}

/**
 * Apply a blob-edit tool (PCB board, schematic sheet, design constraints)
 * through the engine's Rust lowering. A refused edit (no board, index out of
 * range, unknown id) leaves the document untouched; returns whether it applied.
 */
function applyEdit(
  get: () => DocumentState,
  set: (patch: Partial<DocumentState>) => void,
  tool: string,
  args: unknown,
): boolean {
  const engine = get()._crdtEngine;
  if (!engine) return false;
  const envelope = applyToolCall(engine, tool, args);
  if (!envelope.ok) {
    console.warn(`[document-store] ${tool}: ${envelope.error.message}`);
    return false;
  }
  set(applyApiResult(envelope.result));
  return true;
}

/** Create a PCB board feature via an engine tool; returns its board node id.
 *  Throws when the engine refuses (e.g. an unsupported layer count). */
function addBoard(
  get: () => DocumentState,
  set: (patch: Partial<DocumentState>) => void,
  tool: string,
  args: unknown,
): NodeId {
  const engine = get()._crdtEngine;
  if (!engine) {
    console.error("[PCB] Cannot create board: CRDT engine not initialized");
    return 0;
  }
  const envelope = applyToolCall(engine, tool, args);
  if (!envelope.ok) throw new Error(envelope.error.message);
  set(applyApiResult(envelope.result));
  const created = envelope.result.parts.find((p) => p.id === envelope.result.createdFeatureId);
  const board = (created ?? envelope.result.parts.find((p) => p.kind === "pcb-board")) as
    | PcbBoardPartInfo
    | undefined;
  return board?.boardNodeId ?? 0;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export const useDocumentStore = create<DocumentState>((set, get) => ({
  document: createDocument(),
  parts: [],
  partIndex: new Map(),
  consumedParts: {},
  nextNodeId: 1,
  isDirty: false,
  documentId: null,
  documentName: "Untitled",
  lastSavedAt: null,
  isParameterDragging: false,
  isTransientEval: false,
  loonSource: null,

  // CRDT bridge state
  _crdtEngine: null,
  _crdtEngineClass: null,

  _engineClient: new DocumentEngineClient(
    sharedEngineTransport(() => get()._crdtEngine as unknown as HostedEngine | null),
  ),

  dispatch: async (command) => {
    const readOnly = useUiStore.getState().readOnlyShare;
    if (readOnly) {
      // Same contract as the engine proxy's read-only guard: prompt to fork,
      // leave the document alone.
      if (typeof window !== "undefined") {
        window.dispatchEvent(new CustomEvent("vcad:fork-prompt", { detail: readOnly }));
      }
      return undefined;
    }
    const client = get()._engineClient;
    if (!client.state) await client.init();
    const outcome: CommandOutcome = await client.apply(command);
    for (const w of outcome.warnings) console.warn(`[vcad] ${w}`);
    set(
      applyLegacyResult({
        document: outcome.state.document,
        parts: outcome.state.parts as unknown as PartInfo[],
      }),
    );
    return outcome.createdFeatureId;
  },

  _initCrdt: (EngineClass) => {
    // Guard against double-initialization. React StrictMode fires effects
    // twice in dev, and without this guard we'd construct two independent
    // WasmDocumentEngine instances, leak the first, and risk downstream
    // wasm-bindgen borrow / OOB / null-ptr errors if GC frees the orphaned
    // instance while its ptr is still referenced somewhere. If the engine
    // class changes (shouldn't happen in practice), we do recreate.
    const existing = get()._crdtEngine;
    const existingClass = get()._crdtEngineClass;
    if (existing && existingClass === EngineClass) {
      return;
    }
    if (existing) {
      try {
        existing.free();
      } catch {
        /* best effort */
      }
    }
    _sceneSettingsFeatureId = null;
    _schematicFeatureId = null;
    _analysisStudiesFeatureId = null;
    _drawingSettingsFeatureId = null;
    const engine = new EngineClass();
    set({ _crdtEngine: engine, _crdtEngineClass: EngineClass });
  },

  saveCrdt: () => {
    const engine = get()._crdtEngine;
    if (!engine) return null;
    return engine.save();
  },

  loadCrdt: (bytes, EngineClass) => {
    const engine = EngineClass.load(bytes);
    const doc: Document = JSON.parse(engine.get_document_json());
    const parts: PartInfo[] = JSON.parse(engine.get_parts_json());
    const patch = applyLegacyResult({ document: doc, parts });
    set({
      ...patch,
      _crdtEngine: engine,
      _crdtEngineClass: EngineClass,
      isDirty: false,
    });
  },

  pushUndoSnapshot: () => {
    // No-op — CRDT undo handles granularity automatically.
  },

  mergeRemoteOps: (opsJson) => {
    const engine = get()._crdtEngine;
    if (!engine) return false;
    const result = engine.merge_remote(opsJson);
    if (result) {
      // Apply the merged state WITHOUT marking dirty — remote ops are already
      // persisted on the sender's side. Setting isDirty would trigger auto-save
      // and camera-fit side effects we don't want for incoming remote changes.
      const patch = applyLegacyResult(result);
      delete patch.isDirty;
      set(patch);
      return true;
    }
    return false;
  },

  getSyncClock: () => {
    const engine = get()._crdtEngine;
    if (!engine) return "{}";
    return engine.get_sync_clock();
  },

  getOpsSince: (remoteClockJson) => {
    const engine = get()._crdtEngine;
    if (!engine) return "[]";
    return engine.get_ops_since(remoteClockJson);
  },

  addPrimitive: (kind) => {
    const engine = get()._crdtEngine;
    // Guard against a stale wrapper whose underlying Rust value has already
    // been freed. Calling add_feature with __wbg_ptr === 0 passes null into
    // wasm-bindgen and throws "Out of bounds memory access" from WASM, which
    // React's error boundary then catches and blanks the app. Bail early
    // with an empty id so callers see a failed primitive add instead.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (!engine || (engine as any).__wbg_ptr === 0) {
      console.warn("[document-store] addPrimitive: engine is null/freed");
      return "";
    }
    const defaults: Record<PrimitiveKind, object> = {
      cube: { type: "Cube", size_x: 20, size_y: 20, size_z: 20 },
      cylinder: { type: "Cylinder", radius: 10, height: 20, segments: 32 },
      sphere: { type: "Sphere", radius: 10, segments: 32 },
    };
    try {
      const result = applyCommand(engine, { type: "add_feature", feature: defaults[kind] as FeatureInput });
      set(applyApiResult(result));
      return result.createdFeatureId ?? "";
    } catch (e) {
      console.error("[document-store] addPrimitive crashed:", e);
      return "";
    }
  },

  addPartInstance: (path, version, params) => {
    const engine = get()._crdtEngine;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (!engine || (engine as any).__wbg_ptr === 0) {
      console.warn("[document-store] addPartInstance: engine is null/freed");
      return "";
    }
    try {
      const result = applyCommand(engine, { type: "add_feature", feature: {
          type: "PartInstance",
          path,
          version,
          params_json: JSON.stringify(params),
        } });
      set(applyApiResult(result));
      return result.createdFeatureId ?? "";
    } catch (e) {
      console.error("[document-store] addPartInstance crashed:", e);
      return "";
    }
  },

  removePart: async (partId) => {
    await get().dispatch({ type: "delete_feature", id: partId });
  },

  setTranslation: (partId, offset) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "set_translation", id: partId, offset: [offset.x, offset.y, offset.z] });
    set(applyApiResult(result));
  },

  setRotation: (partId, angles) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "set_rotation", id: partId, angles: [angles.x, angles.y, angles.z] });
    set(applyApiResult(result));
  },

  setScale: (partId, factor) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "set_scale", id: partId, factor: [factor.x, factor.y, factor.z] });
    set(applyApiResult(result));
  },

  updatePrimitiveOp: (partId, op) => {
    const engine = get()._crdtEngine;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (!engine || (engine as any).__wbg_ptr === 0 || !partId) {
      console.warn("[document-store] updatePrimitiveOp: engine null/freed or empty partId");
      return;
    }
    const o = op as Record<string, unknown>;
    let lastResult: CrdtMutationResult | undefined;
    if (o.type === "Cube" && "size" in o) {
      const size = o.size as Vec3;
      applyCommand(engine, { type: "set_param", id: partId, key: "size_x", value: crdtF64(size.x) });
      applyCommand(engine, { type: "set_param", id: partId, key: "size_y", value: crdtF64(size.y) });
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "size_z", value: crdtF64(size.z) });
    } else if (o.type === "Cylinder" && "radius" in o) {
      applyCommand(engine, { type: "set_param", id: partId, key: "radius", value: crdtF64(o.radius as number) });
      applyCommand(engine, { type: "set_param", id: partId, key: "height", value: crdtF64(o.height as number) });
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "segments", value: crdtF64(o.segments as number) });
    } else if (o.type === "Sphere" && "radius" in o) {
      applyCommand(engine, { type: "set_param", id: partId, key: "radius", value: crdtF64(o.radius as number) });
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "segments", value: crdtF64(o.segments as number) });
    } else if (o.type === "Cone" && "radius_bottom" in o) {
      applyCommand(engine, { type: "set_param", id: partId, key: "radius_bottom", value: crdtF64(o.radius_bottom as number) });
      applyCommand(engine, { type: "set_param", id: partId, key: "radius_top", value: crdtF64(o.radius_top as number) });
      applyCommand(engine, { type: "set_param", id: partId, key: "height", value: crdtF64(o.height as number) });
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "segments", value: crdtF64(o.segments as number) });
    } else if (o.type === "Torus" && "major_radius" in o) {
      applyCommand(engine, { type: "set_param", id: partId, key: "major_radius", value: crdtF64(o.major_radius as number) });
      applyCommand(engine, { type: "set_param", id: partId, key: "minor_radius", value: crdtF64(o.minor_radius as number) });
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "segments", value: crdtF64(o.segments as number) });
    }
    if (lastResult) {
      set(applyLegacyResult(lastResult));
    }
  },

  updateSweepOp: (partId, updates) => {
    const engine = get()._crdtEngine!;
    let lastResult: CrdtMutationResult | undefined;
    if (updates.twist_angle !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "twist_angle", value: crdtF64(updates.twist_angle) });
    }
    if (updates.scale_start !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "scale_start", value: crdtF64(updates.scale_start) });
    }
    if (updates.scale_end !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "scale_end", value: crdtF64(updates.scale_end) });
    }
    if (updates.path !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "path", value: crdtStr(JSON.stringify(updates.path)) });
    }
    if (updates.orientation !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "orientation", value: crdtF64(updates.orientation) });
    }
    if (updates.path_segments !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "path_segments", value: crdtF64(updates.path_segments) });
    }
    if (updates.arc_segments !== undefined) {
      lastResult = applyCommand(engine, { type: "set_param", id: partId, key: "arc_segments", value: crdtF64(updates.arc_segments) });
    }
    if (lastResult) {
      set(applyLegacyResult(lastResult));
    }
  },

  setFeatureParam: (partId, key, value) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "set_param", id: partId, key: key, value: value });
    set(applyLegacyResult(result));
  },

  updateBooleanType: async (partId, newType) => {
    await get().dispatch({ type: "set_param", id: partId, key: "boolean_type", value: crdtStr(newType) });
  },

  applyBoolean: (type, partIdA, partIdB) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "Boolean",
      boolean_type: type,
      input_a: partIdA,
      input_b: partIdB,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  duplicateParts: (partIds) => {
    const engine = get()._crdtEngine!;
    const featuresJson = engine.get_ordered_features_json();
    const features: { id: string; kind: string; params: Record<string, unknown> }[] = JSON.parse(featuresJson);
    const newIds: string[] = [];
    let lastResult: CrdtMutationResult | undefined;

    for (const partId of partIds) {
      const feature = features.find((f) => f.id === partId);
      if (!feature) continue;

      // Clone params and add +10mm X offset
      const params = { ...feature.params } as Record<string, CrdtValue>;
      const existingOffset = params.offset as { Vec3: [number, number, number] } | undefined;
      if (existingOffset && "Vec3" in existingOffset) {
        params.offset = { Vec3: [existingOffset.Vec3[0] + 10, existingOffset.Vec3[1], existingOffset.Vec3[2]] };
      } else {
        params.offset = { Vec3: [10, 0, 0] };
      }

      // Append " copy" to name if present
      const existingName = params.name as { String: string } | undefined;
      if (existingName && "String" in existingName) {
        params.name = { String: existingName.String + " copy" };
      }

      lastResult = applyCommand(engine, { type: "create_feature", kind: feature.kind, params: params });
      if (lastResult.createdFeatureId) newIds.push(lastResult.createdFeatureId);
    }

    if (lastResult) set(applyLegacyResult(lastResult));
    return newIds;
  },

  loadDocument: (file) => {
    const state = get();
    const EngineClass = state._crdtEngineClass;
    if (!EngineClass) return;

    // Phase 1: build everything off the NEW engine in isolation. If any step
    // throws (migration panic, borrow-check failure inside wasm-bindgen), the
    // old engine is still untouched and the store is never left in a half-
    // installed state.
    let newEngine: WasmDocumentEngine | null = null;
    let patch: Partial<DocumentState> | null = null;
    let loonSource: string | null = null;
    try {
      // Discriminate on `kind`:
      //  - crdt   → load CRDT bytes directly (no migration, no param loss)
      //  - loon   → seed the engine from the evaluated Document (loon is the
      //             source of truth; the CRDT derives from it each load)
      //  - legacy → run the v1 migration path (the bug-prone route — kept
      //             only for reading old files, never written by new saves)
      switch (file.kind) {
        case "crdt":
          newEngine = EngineClass.load(file.crdtBytes);
          break;
        case "loon":
        case "legacy":
          newEngine = EngineClass.from_v1_json(JSON.stringify(file.document));
          break;
      }
      const doc: Document = JSON.parse(newEngine.get_document_json());
      const parts: PartInfo[] = JSON.parse(newEngine.get_parts_json());
      patch = applyLegacyResult({ document: doc, parts });
      loonSource = file.kind === "loon" ? file.loonSource : null;
    } catch (e) {
      console.error("[document-store] loadDocument failed:", e);
      // Clean up the half-constructed new engine if load died mid-read.
      if (newEngine) {
        try {
          newEngine.free();
        } catch {
          /* best effort — the wrapper is dead to us either way */
        }
      }
      return;
    }

    // Phase 2: commit. Install the new engine FIRST so the store is valid
    // the moment any subscriber re-renders, THEN free the old engine. If
    // the old free() throws (the symptom cam hit when legacy examples
    // trigger wasm-bindgen re-entrancy on an in-flight borrow), the store
    // is already pointing at the healthy new engine so undo/redo checks
    // won't crash into a zeroed wrapper.
    const oldEngine = state._crdtEngine;
    set({
      ...patch,
      _crdtEngine: newEngine,
      isDirty: false,
      loonSource,
    });
    if (oldEngine) {
      try {
        oldEngine.free();
      } catch (e) {
        console.warn("Failed to free previous engine (leaked):", e);
      }
    }
    // Seed the parameters store from the raw file (parameters/bindings are
    // outside the CRDT schema for now; we persist them as top-level JSON).
    //
    // Bindings key on node ids, and the v1 migration renumbers every node —
    // so a legacy/loon load must remap them onto the rebuilt document or they
    // point at arbitrary nodes and fail the whole evaluation. `remapBindings`
    // is a no-op for CRDT-native loads, whose ids already match.
    try {
      const src = file.kind === "crdt" ? patch?.document : file.document;
      const rawBindings = (src?.bindings as Bindings) ?? {};
      let bindings = rawBindings;
      if (Object.keys(rawBindings).length > 0) {
        try {
          const remapped = newEngine.remapBindings?.(
            JSON.stringify(rawBindings),
          ) as { bindings: Bindings; dropped: string[] } | undefined;
          bindings = remapped?.bindings ?? rawBindings;
          if (remapped?.dropped?.length) {
            console.warn(
              `[document-store] dropped ${remapped.dropped.length} binding(s) that did not survive migration:`,
              remapped.dropped,
            );
          }
        } catch (e) {
          // Remap is a repair, not a gate: an engine without it (older wasm)
          // still loads, just with the pre-existing dangling-binding bug.
          console.warn("[document-store] binding remap unavailable:", e);
        }
      }
      useParametersStore.getState().reset({
        parameters: (src?.parameters as Record<string, Parameter>) ?? {},
        bindings,
      });
    } catch {
      useParametersStore.getState().reset();
    }
  },

  addFromIR: (generatedDoc, name) => {
    const engine = get()._crdtEngine!;

    if (name && generatedDoc.roots.length > 0) {
      const firstRoot = generatedDoc.roots[0]!;
      const rootNode = generatedDoc.nodes[String(firstRoot.root)];
      if (rootNode) {
        rootNode.name = name;
      }
    }
    const irJson = JSON.stringify(generatedDoc);
    const result = applyCommand(engine, { type: "import_ir", document: JSON.parse(irJson) as Document });
    set({ ...applyLegacyResult(result), isDirty: true });
    // Merge any parameters / bindings from the generated doc into the
    // parameters store (AI-emitted parametric models flow through here).
    if (generatedDoc.parameters || generatedDoc.bindings) {
      const store = useParametersStore.getState();
      for (const [pname, param] of Object.entries(generatedDoc.parameters ?? {})) {
        store.setParameter(pname, param as Parameter);
      }
      for (const [key, expr] of Object.entries(generatedDoc.bindings ?? {})) {
        const colon = key.indexOf(":");
        if (colon > 0) {
          store.setBinding(
            key.slice(0, colon),
            key.slice(colon + 1),
            expr as Expr,
          );
        }
      }
    }
    return null;
  },

  addExtrude: (plane, origin, segments, direction, options) => {
    if (segments.length === 0) return null;
    const engine = get()._crdtEngine;
    // Same guard as addPrimitive: a stale wrapper whose underlying Rust
    // value has already been freed will OOB inside WASM and blow up the
    // error boundary. Bail early instead.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (!engine || (engine as any).__wbg_ptr === 0) {
      console.warn("[document-store] addExtrude: engine is null/freed");
      return null;
    }

    const depth = Math.sqrt(direction.x ** 2 + direction.y ** 2 + direction.z ** 2);
    const dir = depth > 0 ? { x: direction.x / depth, y: direction.y / depth, z: direction.z / depth } : { x: 0, y: 0, z: 1 };
    const input: Record<string, unknown> = {
      type: "Extrude",
      sketch: sketchJson(segments, plane, origin),
      depth,
      direction: [dir.x, dir.y, dir.z],
    };
    if (options?.twist_angle != null) input.twist_angle = options.twist_angle;
    if (options?.scale_end != null) input.scale_end = options.scale_end;
    try {
      const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
      set(applyApiResult(result));
      return result.createdFeatureId ?? null;
    } catch (e) {
      console.error("[document-store] addExtrude crashed:", e);
      return null;
    }
  },

  addRevolve: (plane, origin, segments, axisOrigin, axisDir, angleDeg) => {
    if (segments.length === 0) return null;
    const engine = get()._crdtEngine!;

    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "Revolve",
      sketch: sketchJson(segments, plane, origin),
      axis_origin: [axisOrigin.x, axisOrigin.y, axisOrigin.z],
      axis_dir: [axisDir.x, axisDir.y, axisDir.z],
      angle_deg: angleDeg,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addSweep: (plane, origin, segments, path, options = {}) => {
    if (segments.length === 0) return null;
    const engine = get()._crdtEngine!;

    const input: Record<string, unknown> = {
      type: "Sweep",
      sketch: sketchJson(segments, plane, origin),
      path: JSON.stringify(path),
    };
    if (options.twist_angle != null) input.twist_angle = options.twist_angle;
    if (options.scale_start != null) input.scale_start = options.scale_start;
    if (options.scale_end != null) input.scale_end = options.scale_end;
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addLoft: (profiles, options = {}) => {
    if (profiles.length < 2) return null;
    const engine = get()._crdtEngine!;

    const profileStrs = profiles.map((p) => sketchJson(p.segments, p.plane, p.origin));
    const input: Record<string, unknown> = {
      type: "Loft",
      profiles: profileStrs,
    };
    if (options.closed != null) input.closed = options.closed;
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addImportedMesh: (positions, indices, normals, source) => {
    const engine = get()._crdtEngine!;

    const input: Record<string, unknown> = {
      type: "ImportedMesh",
      positions_json: JSON.stringify(Array.from(positions)),
      indices_json: JSON.stringify(Array.from(indices)),
    };
    if (normals) input.normals_json = JSON.stringify(Array.from(normals));
    if (source) input.source = source;
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? "";
  },

  addEmbroideryPattern: (design, source) => {
    const engine = get()._crdtEngine!;

    const filename = source?.split(/[/\\]/).pop()?.replace(/\.(pes|dst)$/i, "") ?? "Embroidery";
    const params: Record<string, CrdtValue> = {
      design: crdtStr(JSON.stringify(design)),
      name: crdtStr(filename),
    };
    if (source) params.source = crdtStr(source);
    const result = applyCommand(engine, { type: "create_feature", kind: "embroidery-pattern", params: params });
    set(applyLegacyResult(result));
    return result.createdFeatureId ?? "";
  },

  addTextEmbroidery: async (options) => {
    try {
      const wasm = await import("@vcad/kernel-wasm");
      const optionsJson = JSON.stringify({
        stitch_type: options.stitchType ?? "running",
        color: options.color ?? [0, 0, 0],
        stitch_length: options.stitchLength ?? 2.5,
        density: options.density ?? 4.0,
        satin_width: options.satinWidth ?? 3.0,
        fill_angle: options.fillAngle ?? 0,
        letter_spacing: options.letterSpacing ?? 1.0,
        line_spacing: options.lineSpacing ?? 1.2,
        alignment: options.alignment ?? "left",
      });

      const json = wasm.digitizeText(options.text, options.height, optionsJson);
      const result = JSON.parse(json);

      const design: EmbroideryDesign = {
        threads: result.threads,
        stitch_groups: result.stitchPaths.map(
          (sp: { threadIndex: number; points: [number, number][] }) => ({
            thread_index: sp.threadIndex,
            stitches: sp.points,
          }),
        ),
        hoop_width: result.stats.width,
        hoop_height: result.stats.height,
      };

      const partId = get().addEmbroideryPattern(design, "Text Embroidery");
      return { partId, result };
    } catch (err) {
      console.error("Failed to digitize text:", err);
      return null;
    }
  },

  setThreadColor: async (nodeId, threadIdx, color) => {
    const state = get();
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.threads[threadIdx]) d.threads[threadIdx]!.color = color;
    await get().dispatch({ type: "set_param", id: part.id, key: "design", value: crdtStr(JSON.stringify(d)) });
  },

  setThreadName: async (nodeId, threadIdx, name) => {
    const state = get();
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.threads[threadIdx]) d.threads[threadIdx]!.name = name;
    await get().dispatch({ type: "set_param", id: part.id, key: "design", value: crdtStr(JSON.stringify(d)) });
  },

  setStitchGroupFillParams: (nodeId, groupIdx, params) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.stitch_groups[groupIdx]) {
      const group = d.stitch_groups[groupIdx]!;
      group.fill_params = { ...(group.fill_params ?? DEFAULT_FILL_PARAMS), ...params };
    }
    const result = applyCommand(engine, { type: "set_param", id: part.id, key: "design", value: crdtStr(JSON.stringify(d)) });
    set(applyLegacyResult(result));
  },

  optimizeJumpStitches: (nodeId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design || design.stitch_groups.length <= 1) return;

    const d = structuredClone(design);
    const groups = d.stitch_groups;
    const used = new Set<number>();
    const ordered: typeof groups = [];
    const lastPos = (g: typeof groups[number]) => {
      const s = g.stitches;
      return s.length > 0 ? s[s.length - 1]! : [0, 0] as [number, number];
    };
    const firstPos = (g: typeof groups[number]) => {
      const s = g.stitches;
      return s.length > 0 ? s[0]! : [0, 0] as [number, number];
    };
    ordered.push(groups[0]!);
    used.add(0);
    for (let step = 1; step < groups.length; step++) {
      const [lx, ly] = lastPos(ordered[ordered.length - 1]!);
      let bestIdx = -1;
      let bestDist = Infinity;
      for (let i = 0; i < groups.length; i++) {
        if (used.has(i)) continue;
        const [fx, fy] = firstPos(groups[i]!);
        const dd = (fx - lx) ** 2 + (fy - ly) ** 2;
        if (dd < bestDist) { bestDist = dd; bestIdx = i; }
      }
      if (bestIdx >= 0) { ordered.push(groups[bestIdx]!); used.add(bestIdx); }
    }
    d.stitch_groups = ordered;
    const result = applyCommand(engine, { type: "set_param", id: part.id, key: "design", value: crdtStr(JSON.stringify(d)) });
    set(applyLegacyResult(result));
  },

  addFillet: (partId, radius) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "Fillet",
      input: partId,
      radius,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addChamfer: (partId, distance) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "Chamfer",
      input: partId,
      distance,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addShell: (partId, thickness) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "Shell",
      input: partId,
      thickness,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addStitch: async (partId, options) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart || !isStitchEligible(sourcePart)) return null;

    try {
      const wasm = await import("@vcad/kernel-wasm");

      let resultJson: string;

      if (isTextPart(sourcePart)) {
        const textNode = state.document.nodes[String(sourcePart.textNodeId)];
        if (!textNode || textNode.op.type !== "Text2D") return null;
        const textOp = textNode.op;
        const optionsJson = JSON.stringify({
          stitch_type: options.stitchType ?? "running",
          color: options.color ?? [0, 0, 0],
          stitch_length: options.stitchLength ?? 2.5,
          density: options.density ?? 4.0,
          satin_width: options.satinWidth ?? 3.0,
          fill_angle: options.fillAngle ?? 0,
          letter_spacing: textOp.letter_spacing ?? 1.0,
          line_spacing: textOp.line_spacing ?? 1.2,
          alignment: textOp.alignment ?? "left",
        });
        resultJson = wasm.digitizeText(textOp.text, textOp.height, optionsJson);
      } else {
        let sketchNodeId: number | undefined;
        if (isExtrudePart(sourcePart)) {
          const extNode = state.document.nodes[String(sourcePart.extrudeNodeId)];
          if (extNode?.op.type === "Extrude") sketchNodeId = extNode.op.sketch;
        } else if (isRevolvePart(sourcePart)) {
          const revNode = state.document.nodes[String(sourcePart.revolveNodeId)];
          if (revNode?.op.type === "Revolve") sketchNodeId = revNode.op.sketch;
        } else if (isSweepPart(sourcePart)) {
          const sweepNode = state.document.nodes[String(sourcePart.sweepNodeId)];
          if (sweepNode?.op.type === "Sweep") sketchNodeId = sweepNode.op.sketch;
        } else if (isLoftPart(sourcePart)) {
          sketchNodeId = sourcePart.sketchNodeIds[0];
        }

        if (sketchNodeId == null) return null;

        const sketchNode = state.document.nodes[String(sketchNodeId)];
        if (!sketchNode || sketchNode.op.type !== "Sketch2D") return null;

        const segmentsJson = JSON.stringify(sketchNode.op.segments);
        const optionsJson = JSON.stringify({
          stitch_type: options.stitchType ?? "running",
          color: options.color ?? [0, 0, 0],
          stitch_length: options.stitchLength ?? 2.5,
          density: options.density ?? 4.0,
          satin_width: options.satinWidth ?? 3.0,
          fill_angle: options.fillAngle ?? 0,
        });
        resultJson = wasm.digitizeSketch(segmentsJson, optionsJson);
      }

      const result = JSON.parse(resultJson);

      const design: EmbroideryDesign = {
        threads: result.threads,
        stitch_groups: result.stitchPaths.map(
          (sp: { threadIndex: number; points: [number, number][] }) => ({
            thread_index: sp.threadIndex,
            stitches: sp.points,
          }),
        ),
        hoop_width: result.stats.width,
        hoop_height: result.stats.height,
      };

      // Re-read state (async boundary)
      const state2 = get();
      const engine2 = state2._crdtEngine!;
      const params: Record<string, CrdtValue> = {
        design: crdtStr(JSON.stringify(design)),
        name: crdtStr("Stitch"),
      };
      params.input = crdtRef(partId);
      const res = engine2.create_feature("embroidery-pattern", JSON.stringify(params));
      // Delete source feature (consumed)
      engine2.delete_feature(partId);
      set({ ...applyLegacyResult(res), isDirty: true });
      return res.createdFeatureId ?? null;
    } catch (err) {
      console.error("Failed to create stitch:", err);
      return null;
    }
  },

  addLinearPattern: (partId, direction, count, spacing) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "LinearPattern",
      input: partId,
      direction: [direction.x, direction.y, direction.z],
      count,
      spacing,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addCircularPattern: (partId, axisOrigin, axisDir, count, angleDeg) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "CircularPattern",
      input: partId,
      axis_origin: [axisOrigin.x, axisOrigin.y, axisOrigin.z],
      axis_dir: [axisDir.x, axisDir.y, axisDir.z],
      count,
      angle_deg: angleDeg,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addMirror: (partId, plane) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "add_feature", feature: {
      type: "Mirror",
      input: partId,
      plane,
    } });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addText: (options) => {
    const { text, height, depth, alignment, letterSpacing, lineSpacing } = options;
    if (!text.trim()) return null;

    const engine = get()._crdtEngine!;
    const input: Record<string, unknown> = {
      type: "Text",
      text,
      height,
      depth,
    };
    if (alignment) input.alignment = alignment;
    if (letterSpacing !== undefined) input.letter_spacing = letterSpacing;
    if (lineSpacing !== undefined) input.line_spacing = lineSpacing;
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  setPartMaterial: async (partId, materialKey) => {
    await get().dispatch({ type: "set_material", id: partId, material: materialKey });
  },

  renamePart: async (partId, name) => {
    await get().dispatch({ type: "rename", id: partId, name: name });
  },

  // History steps go through the client queue, so an undo pressed while
  // commands are still in flight undoes the newest of them, not an older
  // edit. Whether there is anything to undo is decided when the step runs:
  // "nothing to undo" is a no-op, not an error.
  undo: () => historyStep(get, "undo"),

  redo: () => historyStep(get, "redo"),

  canUndo: () => {
    const engine = get()._crdtEngine;
    return engine ? engine.can_undo() : false;
  },

  canRedo: () => {
    const engine = get()._crdtEngine;
    return engine ? engine.can_redo() : false;
  },

  markSaved: () => {
    set({ isDirty: false, lastSavedAt: Date.now() });
  },

  // Assembly operations
  setInstanceTransform: (instanceId, transform) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "set_param", id: instanceId, key: "transform", value: crdtStr(JSON.stringify(transform)) });
    set(applyLegacyResult(result));
  },

  setInstanceMaterial: async (instanceId, materialKey) => {
    await get().dispatch({ type: "set_param", id: instanceId, key: "material", value: crdtStr(materialKey) });
  },

  setJointState: (jointId, jointState) => {
    const engine = get()._crdtEngine!;
    const result = applyCommand(engine, { type: "set_joint_state", id: jointId, state: jointState });
    set(applyApiResult(result));
  },

  createPartDef: (partId, name) => {
    const engine = get()._crdtEngine!;
    const input: Record<string, unknown> = {
      type: "PartDef",
      source_feature: partId,
    };
    if (name) input.name = name;
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  createInstance: (partDefId, name, transform) => {
    const engine = get()._crdtEngine!;
    const input: Record<string, unknown> = {
      type: "Instance",
      part_def: partDefId,
    };
    if (name) input.name = name;
    if (transform) input.transform = JSON.stringify(transform);
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? "";
  },

  addJoint: (config) => {
    const engine = get()._crdtEngine!;
    const jk = config.kind as Record<string, unknown>;
    const input: Record<string, unknown> = {
      type: "Joint",
      kind: typeof config.kind === "string" ? config.kind : config.kind.type,
      child_instance: config.childInstanceId,
      anchor_a: [config.parentAnchor.x, config.parentAnchor.y, config.parentAnchor.z],
      anchor_b: [config.childAnchor.x, config.childAnchor.y, config.childAnchor.z],
    };
    if (config.parentInstanceId) input.parent_instance = config.parentInstanceId;
    if (config.name) input.name = config.name;
    if (jk.axis) {
      const axis = jk.axis as Vec3;
      input.axis = [axis.x, axis.y, axis.z];
    }
    const result = applyCommand(engine, { type: "add_feature", feature: input as FeatureInput });
    set(applyApiResult(result));
    return result.createdFeatureId ?? "";
  },

  deleteInstance: (instanceId) => {
    const engine = get()._crdtEngine!;
    // Also delete any joints referencing this instance
    const featuresJson = engine.get_ordered_features_json();
    const features: { id: string; kind: string; params: Record<string, unknown> }[] = JSON.parse(featuresJson);
    for (const f of features) {
      if (f.kind === "joint") {
        const pi = f.params.parent_instance as { FeatureRef: string } | undefined;
        const ci = f.params.child_instance as { FeatureRef: string } | undefined;
        if ((pi && pi.FeatureRef === instanceId) || (ci && ci.FeatureRef === instanceId)) {
          applyCommand(engine, { type: "delete_feature", id: f.id });
        }
      }
    }
    const result = applyCommand(engine, { type: "delete_feature", id: instanceId });
    set(applyApiResult(result));
  },

  deleteJoint: async (jointId) => {
    await get().dispatch({ type: "delete_feature", id: jointId });
  },

  setGroundInstance: (instanceId) => {
    const state = get();
    const engine = state._crdtEngine!;
    // Unset previous ground
    if (state.document.groundInstanceId) {
      const featuresJson = engine.get_ordered_features_json();
      const features: { id: string; kind: string }[] = JSON.parse(featuresJson);
      const oldGround = features.find((f) => f.kind === "instance" && f.id !== instanceId);
      if (oldGround) {
        applyCommand(engine, { type: "set_param", id: oldGround.id, key: "is_ground", value: crdtBool(false) });
      }
    }
    const result = applyCommand(engine, { type: "set_param", id: instanceId, key: "is_ground", value: crdtBool(true) });
    set(applyLegacyResult(result));
  },

  renameInstance: async (instanceId, name) => {
    await get().dispatch({ type: "set_param", id: instanceId, key: "name", value: crdtStr(name) });
  },

  setDocumentMeta: (id, name) => {
    set({ documentId: id, documentName: name });
  },

  setDocumentName: (name) => {
    set({ documentName: name, isDirty: true });
  },

  newDocument: (id, name) => {
    const state = get();
    // A fresh engine is the source of truth; the store's document is its view.
    let engine = state._crdtEngine;
    let view: Partial<DocumentState> = {
      document: createDocument(),
      parts: [],
      partIndex: new Map(),
      consumedParts: {},
      nextNodeId: 1,
    };
    if (state._crdtEngineClass) {
      _sceneSettingsFeatureId = null;
      _schematicFeatureId = null;
      _analysisStudiesFeatureId = null;
      _drawingSettingsFeatureId = null;
      engine = new state._crdtEngineClass();
      view = applyLegacyResult({
        document: JSON.parse(engine.get_document_json()),
        parts: JSON.parse(engine.get_parts_json()),
      });
    }
    set({
      ...view,
      isDirty: false,
      documentId: id,
      documentName: name,
      lastSavedAt: null,
      isParameterDragging: false,
      isTransientEval: false,
      loonSource: null,
      _crdtEngine: engine,
    });
  },

  clearDirtyNodes: () => {
    // No-op — CRDT replaces the document wholesale.
    return new Set<NodeId>();
  },

  setParameterDragging: (dragging) => {
    set({ isParameterDragging: dragging });
  },

  setTransientEval: (active) => {
    set({ isTransientEval: active });
  },

  applyTool: (tool, args) => {
    const engine = get()._crdtEngine;
    if (!engine) return null;
    const envelope = applyToolCall(engine, tool, args);
    if (envelope?.ok) set(applyApiResult(envelope.result));
    return envelope;
  },

  setPartVisible: async (partId, visible) => {
    await get().dispatch({ type: "set_visible", id: partId, visible: visible });
  },

  reorderPart: (partId, newIndex) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.partIndex.get(partId);
    if (!part) return;

    const oldIndex = state.parts.findIndex((p) => p.id === partId);
    if (oldIndex === -1 || oldIndex === newIndex) return;

    const result = applyCommand(engine, { type: "reorder", id: partId, index: newIndex });
    set(applyLegacyResult(result));
  },

  // Analyze mode: studies live in a singleton "analysis-studies" CRDT
  // feature so they survive the canonical v0.4 save (unlike clearance_specs,
  // which only round-trips the server JSON path today).
  setAnalysisStudies: (studies) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateAnalysisStudiesFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "studies", value: crdtStr(JSON.stringify(studies)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  // Scene settings actions
  setSceneSettings: (settings) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    if (settings.environment) applyCommand(engine, { type: "set_param", id: fid, key: "environment", value: crdtStr(JSON.stringify(settings.environment)) });
    if (settings.lights) applyCommand(engine, { type: "set_param", id: fid, key: "lights", value: crdtStr(JSON.stringify(settings.lights)) });
    if (settings.background) applyCommand(engine, { type: "set_param", id: fid, key: "background", value: crdtStr(JSON.stringify(settings.background)) });
    if (settings.postProcessing) applyCommand(engine, { type: "set_param", id: fid, key: "post_processing", value: crdtStr(JSON.stringify(settings.postProcessing)) });
    if (settings.cameraPresets) applyCommand(engine, { type: "set_param", id: fid, key: "camera_presets", value: crdtStr(JSON.stringify(settings.cameraPresets)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  setDrawingSettings: (settings) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateDrawingFeature(state);
    if (settings.titleBlock) applyCommand(engine, { type: "set_param", id: fid, key: "title_block", value: crdtStr(JSON.stringify(settings.titleBlock)) });
    if (settings.sections) applyCommand(engine, { type: "set_param", id: fid, key: "sections", value: crdtStr(JSON.stringify(settings.sections)) });
    if (settings.showBom !== undefined) applyCommand(engine, { type: "set_param", id: fid, key: "show_bom", value: crdtStr(JSON.stringify(settings.showBom)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateEnvironment: (environment) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "environment", value: crdtStr(JSON.stringify(environment)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateLights: (lights) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "lights", value: crdtStr(JSON.stringify(lights)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addLight: (light) => {
    const state = get();
    const engine = state._crdtEngine!;
    const lights = [...(state.document.scene?.lights ?? []), light];
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "lights", value: crdtStr(JSON.stringify(lights)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeLight: (lightId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const lights = (state.document.scene?.lights ?? []).filter((l) => l.id !== lightId);
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "lights", value: crdtStr(JSON.stringify(lights)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateLight: (lightId, updates) => {
    const state = get();
    const engine = state._crdtEngine!;
    const lights = (state.document.scene?.lights ?? []).map((l) =>
      l.id === lightId ? { ...l, ...updates } : l,
    );
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "lights", value: crdtStr(JSON.stringify(lights)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateBackground: (background) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "background", value: crdtStr(JSON.stringify(background)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updatePostProcessing: (postProcessing) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "post_processing", value: crdtStr(JSON.stringify(postProcessing)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addCameraPreset: (preset) => {
    const state = get();
    const engine = state._crdtEngine!;
    const presets = [...(state.document.scene?.cameraPresets ?? []), preset];
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "camera_presets", value: crdtStr(JSON.stringify(presets)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeCameraPreset: (presetId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const presets = (state.document.scene?.cameraPresets ?? []).filter((p) => p.id !== presetId);
    const fid = getOrCreateSceneFeature(state);
    applyCommand(engine, { type: "set_param", id: fid, key: "camera_presets", value: crdtStr(JSON.stringify(presets)) });
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  // =========================================================================
  // Electronics (ECAD) mutations
  // =========================================================================

  initSchematic: (title) => {
    const state = get();
    const schId = getOrCreateSchematicFeature(state);
    const sheet = { title: title ?? "Sheet 1", components: [], wires: [], junctions: [], labels: [] };
    const result = applyCommand(state._crdtEngine!, { type: "set_param", id: schId, key: "sheet", value: crdtStr(JSON.stringify(sheet)) });
    set(applyLegacyResult(result));
  },

  initPcb: (options) => {
    // The engine builds the board (stackup, default rules) and refuses an
    // unsupported layer count.
    return addBoard(get, set, "pcb_init", {
      width: options?.width,
      height: options?.height,
      layers: options?.layers,
      thickness: options?.thickness,
      trace_width: options?.traceWidth,
      clearance: options?.clearance,
      name: options?.name,
    });
  },

  importPcb: (pcb, name) => {
    return addBoard(get, set, "pcb_import", { board: pcb, name: name ?? "Imported PCB" });
  },

  syncSchematicToPcb: (_boardNodeId, netlist, opts) => {
    const { pcb, schematic } = get().document;
    if (!pcb || !schematic) return;
    // Reconciliation (placement + net mapping) runs in the engine; an
    // in-sync board is an empty edit.
    applyEdit(get, set, "pcb_sync_schematic", { netlist, place_unplaced: opts?.placeUnplaced });
  },

  moveSchematicComponent: (idx, position) => {
    applyEdit(get, set, "sch_move_component", { index: idx, position: { x: position.x, y: position.y } });
  },

  moveSchematicComponentWithWires: (idx, position, wireUpdates) => {
    applyEdit(get, set, "sch_move_component", {
      index: idx,
      position: { x: position.x, y: position.y },
      wire_updates: wireUpdates.map((wu) => ({
        wire_index: wu.wireIdx,
        endpoint: wu.endpoint,
        position: { x: wu.pos.x, y: wu.pos.y },
      })),
    });
  },

  moveFootprint: (_nodeId, idx, position) => {
    // The engine drags trace endpoints on the footprint's pads along with it.
    applyEdit(get, set, "pcb_move_footprint", { index: idx, position: { x: position.x, y: position.y } });
  },

  rotateFootprint: (_nodeId, idx, angleDeg) => {
    applyEdit(get, set, "pcb_rotate_footprint", { index: idx, angle: angleDeg });
  },

  flipFootprint: (_nodeId, idx) => {
    applyEdit(get, set, "pcb_flip_footprint", { index: idx });
  },

  addTrace: (_nodeId, trace) => {
    applyEdit(get, set, "pcb_add_trace", {
      trace: {
        start: { x: trace.start.x, y: trace.start.y },
        end: { x: trace.end.x, y: trace.end.y },
        width: trace.width, layer: trace.layer, net: trace.net,
      },
    });
  },

  removeTrace: (_nodeId, idx) => {
    applyEdit(get, set, "pcb_remove_trace", { index: idx });
  },

  addVia: (_nodeId, via) => {
    applyEdit(get, set, "pcb_add_via", {
      via: {
        position: { x: via.position.x, y: via.position.y },
        diameter: via.diameter, drill: via.drill,
        startLayer: via.startLayer, endLayer: via.endLayer, net: via.net,
      },
    });
  },

  removeVia: (_nodeId, idx) => {
    applyEdit(get, set, "pcb_remove_via", { index: idx });
  },

  addSchematicComponent: (comp, _boardNodeId) => {
    // The engine also places the component's footprint template on the board.
    applyEdit(get, set, "sch_add_component", { component: comp });
  },

  removeSchematicComponent: (idx, _boardNodeId) => {
    applyEdit(get, set, "sch_remove_component", { index: idx });
  },

  updateSchematicComponent: (idx, updates, _boardNodeId) => {
    applyEdit(get, set, "sch_update_component", { index: idx, updates });
  },

  addSchematicWire: (wire) => {
    applyEdit(get, set, "sch_add_wire", { wire: { start: { ...wire.start }, end: { ...wire.end } } });
  },

  removeSchematicWire: (idx) => {
    applyEdit(get, set, "sch_remove_wire", { index: idx });
  },

  addSchematicLabel: (label) => {
    applyEdit(get, set, "sch_add_label", { label });
  },

  removeSchematicLabel: (idx) => {
    applyEdit(get, set, "sch_remove_label", { index: idx });
  },

  addSchematicJunction: (junction) => {
    applyEdit(get, set, "sch_add_junction", { junction: { position: { ...junction.position } } });
  },

  addFootprint: (_nodeId, fp) => {
    applyEdit(get, set, "pcb_add_footprint", { footprint: fp });
  },

  removeFootprint: (_nodeId, idx) => {
    applyEdit(get, set, "pcb_remove_footprint", { index: idx });
  },

  setBoardOutline: (outline) => {
    applyEdit(get, set, "pcb_set_outline", { outline });
  },

  resizeBoard: (width, height) => {
    applyEdit(get, set, "pcb_resize", { width, height });
  },

  persistSketchConstraints: (partId, session) => {
    const state = get();
    const part = state.partIndex.get(partId) as { sketchNodeId?: NodeId } | undefined;
    const sketchNode = part?.sketchNodeId;
    if (sketchNode == null) return;
    const next = mergeSketchConstraints(
      state.document.constraints ?? [],
      sketchNode,
      session,
    );
    if (next.length === 0 && (state.document.constraints ?? []).length === 0) return;
    applyEdit(get, set, "set_design_constraints", { constraints: next });
  },

  addDesignConstraint: (constraint) => {
    // The engine assigns the next free `cN` id.
    if (!applyEdit(get, set, "add_design_constraint", { constraint })) return null;
    return get().solveDesignConstraints();
  },

  removeDesignConstraint: (id) => {
    // Deleting only frees degrees of freedom — no re-solve needed.
    applyEdit(get, set, "remove_design_constraint", { id });
  },

  updateDesignConstraint: (id, patch) => {
    if (!applyEdit(get, set, "update_design_constraint", { id, value: patch.value, driven: patch.driven })) return null;
    return get().solveDesignConstraints();
  },

  solveDesignConstraints: (opts) => {
    const state = get();
    if ((state.document.constraints ?? []).length === 0) return null;
    // Same guard pattern as sketch solving: WASM may not be hydrated in
    // tests — degrade to a no-op rather than crash.
    const wasm = getKernelWasmSync() as unknown as {
      solveDesignConstraints?: (doc: string, opts: string) => string;
    } | null;
    if (!wasm || typeof wasm.solveDesignConstraints !== "function") return null;
    try {
      const resultJson = wasm.solveDesignConstraints(
        JSON.stringify(state.document),
        JSON.stringify(opts?.extraFixed ? { extraFixed: opts.extraFixed } : {}),
      );
      const result = JSON.parse(resultJson) as {
        document: Document;
        report: DesignConstraintSolveReport;
      };
      const { report } = result;
      // The engine commits the solve: a converged PCB group's moved board
      // and back-annotated driven values.
      applyEdit(get, set, "apply_design_solve", { document: result.document, report });
      return report;
    } catch (e) {
      console.warn("[document-store] solveDesignConstraints failed:", e);
      return null;
    }
  },
}));
