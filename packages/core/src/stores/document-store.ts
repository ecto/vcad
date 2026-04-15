import { create } from "zustand";
import type {
  Document,
  NodeId,
  Vec3,
  SketchSegment2D,
  PathCurve,
  Transform3D,
  SweepOp,
  JointKind,
  SceneSettings,
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
  EmbroideryDesign,
  FillParams,
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
  // Typed API (new) — returns ApiResult with consumedPartIds
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

export interface VcadFile {
  document: Document;
  parts: PartInfo[];
  consumedParts?: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum?: number;
  loonSource?: string | null;
}

export interface PcbCreateOptions {
  width?: number;        // mm, default 50
  height?: number;       // mm, default 30
  layers?: 2 | 4 | 6;   // default 2
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

  /** Loon source code — non-null when document was loaded from loon format. */
  loonSource: string | null;

  // --------------- CRDT engine ---------------
  /** The CRDT engine instance, or null if not yet initialized. */
  _crdtEngine: WasmDocumentEngine | null;
  /** The CRDT engine constructor (stored for creating new engines). */
  _crdtEngineClass: WasmDocumentEngineConstructor | null;
  /** Initialize the CRDT engine. Called once after WASM loads. */
  _initCrdt: (EngineClass: WasmDocumentEngineConstructor) => void;
  /** Save CRDT document to bytes (returns null if engine not initialized). */
  saveCrdt: () => Uint8Array | null;
  /** Load CRDT document from bytes. */
  loadCrdt: (
    bytes: Uint8Array,
    EngineClass: WasmDocumentEngineConstructor,
  ) => void;

  // mutations
  addPrimitive: (kind: PrimitiveKind) => string;
  removePart: (partId: string) => void;
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
  renamePart: (partId: string, name: string) => void;
  updateBooleanType: (
    partId: string,
    newType: BooleanType,
    skipUndo?: boolean,
  ) => void;
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
  setPartMaterial: (partId: string, materialKey: string) => void;
  /** No-op — CRDT undo handles granularity automatically. */
  pushUndoSnapshot: () => void;
  undo: () => void;
  redo: () => void;
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
  setInstanceMaterial: (instanceId: string, materialKey: string) => void;
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
  deleteJoint: (jointId: string) => void;
  setGroundInstance: (instanceId: string) => void;
  renameInstance: (instanceId: string, name: string) => void;
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
  setThreadColor: (nodeId: NodeId, threadIdx: number, color: [number, number, number]) => void;
  setThreadName: (nodeId: NodeId, threadIdx: number, name: string) => void;
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
  // Visibility toggle
  setPartVisible: (partId: string, visible: boolean) => void;
  // Reorder parts in tree
  reorderPart: (partId: string, newIndex: number) => void;
  // Scene settings actions
  setSceneSettings: (settings: SceneSettings) => void;
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
  syncSchematicToPcb: (boardNodeId: NodeId) => void;
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

/** Cached singleton feature IDs (lazily created, cleared on engine init). */
let _sceneSettingsFeatureId: string | null = null;
let _schematicFeatureId: string | null = null;

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

  const result = engine.create_feature("scene-settings", "{}");
  if (result.createdFeatureId) {
    _sceneSettingsFeatureId = result.createdFeatureId;
    return result.createdFeatureId;
  }
  return "";
}

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

  const result = engine.create_feature("schematic", "{}");
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

/** Write the entire schematic sheet back to the CRDT feature. */
function setCrdtSchematic(state: DocumentState, schematic: NonNullable<Document["schematic"]>): void {
  const schId = getOrCreateSchematicFeature(state);
  state._crdtEngine!.set_param(schId, "sheet", JSON.stringify(crdtStr(JSON.stringify(schematic))));
}

/** Write the entire PCB board back to the CRDT pcb-board feature. */
function setCrdtPcb(state: DocumentState, pcb: Pcb): Partial<DocumentState> {
  const pcbFid = getPcbBoardFeatureId(state);
  if (!pcbFid) return {};
  const result = state._crdtEngine!.set_param(pcbFid, "board", JSON.stringify(crdtStr(JSON.stringify(pcb))));
  return applyLegacyResult(result);
}

/** Shared logic for adding a Pcb board to the document. */
function addPcbToDocument(
  get: () => DocumentState,
  set: (s: Partial<DocumentState>) => void,
  pcbBoard: Pcb,
  boardName: string,
): NodeId {
  const engine = get()._crdtEngine;
  if (!engine) {
    console.error("[PCB] Cannot create board: CRDT engine not initialized");
    return 0;
  }
  const params: Record<string, CrdtValue> = {
    name: crdtStr(boardName),
    board: crdtStr(JSON.stringify(pcbBoard)),
    material: crdtStr("__pcb_fr4__"),
  };
  const result = engine.create_feature("pcb-board", JSON.stringify(params));
  set({ ...applyLegacyResult(result), isDirty: true });
  const pcbPart = result.parts.find((p) => p.kind === "pcb-board");
  return pcbPart ? (pcbPart as PcbBoardPartInfo).boardNodeId : 0;
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
  loonSource: null,

  // CRDT bridge state
  _crdtEngine: null,
  _crdtEngineClass: null,

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
      const result = engine.add_feature(JSON.stringify(defaults[kind]));
      set(applyApiResult(result));
      return result.createdFeatureId ?? "";
    } catch (e) {
      console.error("[document-store] addPrimitive crashed:", e);
      return "";
    }
  },

  removePart: (partId) => {
    const engine = get()._crdtEngine!;
    const result = engine.delete_feature_by_id(partId);
    set(applyApiResult(result));
  },

  setTranslation: (partId, offset) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_translation(partId, offset.x, offset.y, offset.z);
    set(applyApiResult(result));
  },

  setRotation: (partId, angles) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_rotation(partId, angles.x, angles.y, angles.z);
    set(applyApiResult(result));
  },

  setScale: (partId, factor) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_scale(partId, factor.x, factor.y, factor.z);
    set(applyApiResult(result));
  },

  updatePrimitiveOp: (partId, op) => {
    const engine = get()._crdtEngine!;
    const o = op as Record<string, unknown>;
    let lastResult: CrdtMutationResult | undefined;
    if (o.type === "Cube" && "size" in o) {
      const size = o.size as Vec3;
      engine.set_param(partId, "size_x", JSON.stringify(crdtF64(size.x)));
      engine.set_param(partId, "size_y", JSON.stringify(crdtF64(size.y)));
      lastResult = engine.set_param(partId, "size_z", JSON.stringify(crdtF64(size.z)));
    } else if (o.type === "Cylinder" && "radius" in o) {
      engine.set_param(partId, "radius", JSON.stringify(crdtF64(o.radius as number)));
      engine.set_param(partId, "height", JSON.stringify(crdtF64(o.height as number)));
      lastResult = engine.set_param(partId, "segments", JSON.stringify(crdtF64(o.segments as number)));
    } else if (o.type === "Sphere" && "radius" in o) {
      engine.set_param(partId, "radius", JSON.stringify(crdtF64(o.radius as number)));
      lastResult = engine.set_param(partId, "segments", JSON.stringify(crdtF64(o.segments as number)));
    } else if (o.type === "Cone" && "radius_bottom" in o) {
      engine.set_param(partId, "radius_bottom", JSON.stringify(crdtF64(o.radius_bottom as number)));
      engine.set_param(partId, "radius_top", JSON.stringify(crdtF64(o.radius_top as number)));
      engine.set_param(partId, "height", JSON.stringify(crdtF64(o.height as number)));
      lastResult = engine.set_param(partId, "segments", JSON.stringify(crdtF64(o.segments as number)));
    }
    if (lastResult) {
      set(applyLegacyResult(lastResult));
    }
  },

  updateSweepOp: (partId, updates) => {
    const engine = get()._crdtEngine!;
    let lastResult: CrdtMutationResult | undefined;
    if (updates.twist_angle !== undefined) {
      lastResult = engine.set_param(partId, "twist_angle", JSON.stringify(crdtF64(updates.twist_angle)));
    }
    if (updates.scale_start !== undefined) {
      lastResult = engine.set_param(partId, "scale_start", JSON.stringify(crdtF64(updates.scale_start)));
    }
    if (updates.scale_end !== undefined) {
      lastResult = engine.set_param(partId, "scale_end", JSON.stringify(crdtF64(updates.scale_end)));
    }
    if (updates.path !== undefined) {
      lastResult = engine.set_param(partId, "path", JSON.stringify(crdtStr(JSON.stringify(updates.path))));
    }
    if (updates.orientation !== undefined) {
      lastResult = engine.set_param(partId, "orientation", JSON.stringify(crdtF64(updates.orientation)));
    }
    if (updates.path_segments !== undefined) {
      lastResult = engine.set_param(partId, "path_segments", JSON.stringify(crdtF64(updates.path_segments)));
    }
    if (updates.arc_segments !== undefined) {
      lastResult = engine.set_param(partId, "arc_segments", JSON.stringify(crdtF64(updates.arc_segments)));
    }
    if (lastResult) {
      set(applyLegacyResult(lastResult));
    }
  },

  setFeatureParam: (partId, key, value) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_param(partId, key, JSON.stringify(value));
    set(applyLegacyResult(result));
  },

  updateBooleanType: (partId, newType) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_param(partId, "boolean_type", JSON.stringify(crdtStr(newType)));
    set(applyLegacyResult(result));
  },

  applyBoolean: (type, partIdA, partIdB) => {
    const engine = get()._crdtEngine!;
    const result = engine.add_feature(JSON.stringify({
      type: "Boolean",
      boolean_type: type,
      input_a: partIdA,
      input_b: partIdB,
    }));
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

      lastResult = engine.create_feature(feature.kind, JSON.stringify(params));
      if (lastResult.createdFeatureId) newIds.push(lastResult.createdFeatureId);
    }

    if (lastResult) set(applyLegacyResult(lastResult));
    return newIds;
  },

  loadDocument: (file) => {
    const state = get();
    if (!state._crdtEngineClass) return;

    // Phase 1: build everything off the NEW engine in isolation. If any step
    // throws (migration panic, borrow-check failure inside wasm-bindgen), the
    // old engine is still untouched and the store is never left in a half-
    // installed state.
    let newEngine: WasmDocumentEngine | null = null;
    let patch: Partial<DocumentState> | null = null;
    try {
      const irJson = JSON.stringify(file.document);
      newEngine = state._crdtEngineClass.from_v1_json(irJson);
      const doc: Document = JSON.parse(newEngine.get_document_json());
      const parts: PartInfo[] = JSON.parse(newEngine.get_parts_json());
      patch = applyLegacyResult({ document: doc, parts });
    } catch (e) {
      console.error("Failed to migrate legacy document to CRDT:", e);
      // Clean up the half-constructed new engine if migration died mid-read.
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
      loonSource: file.loonSource ?? null,
    });
    if (oldEngine) {
      try {
        oldEngine.free();
      } catch (e) {
        console.warn("Failed to free previous engine (leaked):", e);
      }
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
    const result = engine.import_ir(irJson);
    set({ ...applyLegacyResult(result), isDirty: true });
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
      const result = engine.add_feature(JSON.stringify(input));
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

    const result = engine.add_feature(JSON.stringify({
      type: "Revolve",
      sketch: sketchJson(segments, plane, origin),
      axis_origin: [axisOrigin.x, axisOrigin.y, axisOrigin.z],
      axis_dir: [axisDir.x, axisDir.y, axisDir.z],
      angle_deg: angleDeg,
    }));
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
    const result = engine.add_feature(JSON.stringify(input));
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
    const result = engine.add_feature(JSON.stringify(input));
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
    const result = engine.add_feature(JSON.stringify(input));
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
    const result = engine.create_feature("embroidery-pattern", JSON.stringify(params));
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

  setThreadColor: (nodeId, threadIdx, color) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.threads[threadIdx]) d.threads[threadIdx]!.color = color;
    const result = engine.set_param(part.id, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyLegacyResult(result));
  },

  setThreadName: (nodeId, threadIdx, name) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.threads[threadIdx]) d.threads[threadIdx]!.name = name;
    const result = engine.set_param(part.id, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyLegacyResult(result));
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
    const result = engine.set_param(part.id, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
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
    const result = engine.set_param(part.id, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyLegacyResult(result));
  },

  addFillet: (partId, radius) => {
    const engine = get()._crdtEngine!;
    const result = engine.add_feature(JSON.stringify({
      type: "Fillet",
      input: partId,
      radius,
    }));
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addChamfer: (partId, distance) => {
    const engine = get()._crdtEngine!;
    const result = engine.add_feature(JSON.stringify({
      type: "Chamfer",
      input: partId,
      distance,
    }));
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addShell: (partId, thickness) => {
    const engine = get()._crdtEngine!;
    const result = engine.add_feature(JSON.stringify({
      type: "Shell",
      input: partId,
      thickness,
    }));
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
    const result = engine.add_feature(JSON.stringify({
      type: "LinearPattern",
      input: partId,
      direction: [direction.x, direction.y, direction.z],
      count,
      spacing,
    }));
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addCircularPattern: (partId, axisOrigin, axisDir, count, angleDeg) => {
    const engine = get()._crdtEngine!;
    const result = engine.add_feature(JSON.stringify({
      type: "CircularPattern",
      input: partId,
      axis_origin: [axisOrigin.x, axisOrigin.y, axisOrigin.z],
      axis_dir: [axisDir.x, axisDir.y, axisDir.z],
      count,
      angle_deg: angleDeg,
    }));
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  addMirror: (partId, plane) => {
    const engine = get()._crdtEngine!;
    const result = engine.add_feature(JSON.stringify({
      type: "Mirror",
      input: partId,
      plane,
    }));
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
    const result = engine.add_feature(JSON.stringify(input));
    set(applyApiResult(result));
    return result.createdFeatureId ?? null;
  },

  setPartMaterial: (partId, materialKey) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_material(partId, materialKey);
    set(applyApiResult(result));
  },

  renamePart: (partId, name) => {
    const engine = get()._crdtEngine!;
    const result = engine.rename_feature(partId, name);
    set(applyApiResult(result));
  },

  undo: () => {
    const engine = get()._crdtEngine!;
    if (engine.can_undo()) {
      const result = engine.undo();
      set(applyLegacyResult(result));
    }
  },

  redo: () => {
    const engine = get()._crdtEngine!;
    if (engine.can_redo()) {
      const result = engine.redo();
      set(applyLegacyResult(result));
    }
  },

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
    const result = engine.set_param(instanceId, "transform", JSON.stringify(crdtStr(JSON.stringify(transform))));
    set(applyLegacyResult(result));
  },

  setInstanceMaterial: (instanceId, materialKey) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_param(instanceId, "material", JSON.stringify(crdtStr(materialKey)));
    set(applyLegacyResult(result));
  },

  setJointState: (jointId, jointState) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_joint_state(jointId, jointState);
    set(applyApiResult(result));
  },

  createPartDef: (partId, name) => {
    const engine = get()._crdtEngine!;
    const input: Record<string, unknown> = {
      type: "PartDef",
      source_feature: partId,
    };
    if (name) input.name = name;
    const result = engine.add_feature(JSON.stringify(input));
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
    const result = engine.add_feature(JSON.stringify(input));
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
    const result = engine.add_feature(JSON.stringify(input));
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
          engine.delete_feature_by_id(f.id);
        }
      }
    }
    const result = engine.delete_feature_by_id(instanceId);
    set(applyApiResult(result));
  },

  deleteJoint: (jointId) => {
    const engine = get()._crdtEngine!;
    const result = engine.delete_feature_by_id(jointId);
    set(applyApiResult(result));
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
        engine.set_param(oldGround.id, "is_ground", JSON.stringify(crdtBool(false)));
      }
    }
    const result = engine.set_param(instanceId, "is_ground", JSON.stringify(crdtBool(true)));
    set(applyLegacyResult(result));
  },

  renameInstance: (instanceId, name) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_param(instanceId, "name", JSON.stringify(crdtStr(name)));
    set(applyLegacyResult(result));
  },

  setDocumentMeta: (id, name) => {
    set({ documentId: id, documentName: name });
  },

  setDocumentName: (name) => {
    set({ documentName: name, isDirty: true });
  },

  newDocument: (id, name) => {
    const state = get();
    // Create a fresh CRDT engine if constructor is available
    if (state._crdtEngineClass) {
      _sceneSettingsFeatureId = null;
      _schematicFeatureId = null;
      const engine = new state._crdtEngineClass();
      set({
        document: createDocument(),
        parts: [],
        partIndex: new Map(),
        consumedParts: {},
        nextNodeId: 1,
        isDirty: false,
        documentId: id,
        documentName: name,
        lastSavedAt: null,
        isParameterDragging: false,
        loonSource: null,
        _crdtEngine: engine,
      });
    } else {
      set({
        document: createDocument(),
        parts: [],
        partIndex: new Map(),
        consumedParts: {},
        nextNodeId: 1,
        isDirty: false,
        documentId: id,
        documentName: name,
        lastSavedAt: null,
        isParameterDragging: false,
        loonSource: null,
      });
    }
  },

  clearDirtyNodes: () => {
    // No-op — CRDT replaces the document wholesale.
    return new Set<NodeId>();
  },

  setParameterDragging: (dragging) => {
    set({ isParameterDragging: dragging });
  },

  setPartVisible: (partId, visible) => {
    const engine = get()._crdtEngine!;
    const result = engine.set_visible(partId, visible);
    set(applyApiResult(result));
  },

  reorderPart: (partId, newIndex) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.partIndex.get(partId);
    if (!part) return;

    const oldIndex = state.parts.findIndex((p) => p.id === partId);
    if (oldIndex === -1 || oldIndex === newIndex) return;

    const featuresJson = engine.get_ordered_features_json();
    const features: { id: string }[] = JSON.parse(featuresJson);
    const others = features.filter((f) => f.id !== partId);

    const beforeId = newIndex > 0 ? others[newIndex - 1]?.id ?? "" : "";
    const afterId = newIndex < others.length ? others[newIndex]?.id ?? "" : "";

    const positionJson = engine.compute_position_between(beforeId, afterId);
    const result = engine.move_feature(partId, positionJson);
    set(applyLegacyResult(result));
  },

  // Scene settings actions
  setSceneSettings: (settings) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    if (settings.environment) engine.set_param(fid, "environment", JSON.stringify(crdtStr(JSON.stringify(settings.environment))));
    if (settings.lights) engine.set_param(fid, "lights", JSON.stringify(crdtStr(JSON.stringify(settings.lights))));
    if (settings.background) engine.set_param(fid, "background", JSON.stringify(crdtStr(JSON.stringify(settings.background))));
    if (settings.postProcessing) engine.set_param(fid, "post_processing", JSON.stringify(crdtStr(JSON.stringify(settings.postProcessing))));
    if (settings.cameraPresets) engine.set_param(fid, "camera_presets", JSON.stringify(crdtStr(JSON.stringify(settings.cameraPresets))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateEnvironment: (environment) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "environment", JSON.stringify(crdtStr(JSON.stringify(environment))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateLights: (lights) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "lights", JSON.stringify(crdtStr(JSON.stringify(lights))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addLight: (light) => {
    const state = get();
    const engine = state._crdtEngine!;
    const lights = [...(state.document.scene?.lights ?? []), light];
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "lights", JSON.stringify(crdtStr(JSON.stringify(lights))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeLight: (lightId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const lights = (state.document.scene?.lights ?? []).filter((l) => l.id !== lightId);
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "lights", JSON.stringify(crdtStr(JSON.stringify(lights))));
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
    engine.set_param(fid, "lights", JSON.stringify(crdtStr(JSON.stringify(lights))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateBackground: (background) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "background", JSON.stringify(crdtStr(JSON.stringify(background))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updatePostProcessing: (postProcessing) => {
    const state = get();
    const engine = state._crdtEngine!;
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "post_processing", JSON.stringify(crdtStr(JSON.stringify(postProcessing))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addCameraPreset: (preset) => {
    const state = get();
    const engine = state._crdtEngine!;
    const presets = [...(state.document.scene?.cameraPresets ?? []), preset];
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "camera_presets", JSON.stringify(crdtStr(JSON.stringify(presets))));
    const doc: Document = JSON.parse(engine.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeCameraPreset: (presetId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const presets = (state.document.scene?.cameraPresets ?? []).filter((p) => p.id !== presetId);
    const fid = getOrCreateSceneFeature(state);
    engine.set_param(fid, "camera_presets", JSON.stringify(crdtStr(JSON.stringify(presets))));
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
    const result = state._crdtEngine!.set_param(schId, "sheet", JSON.stringify(crdtStr(JSON.stringify(sheet))));
    set(applyLegacyResult(result));
  },

  initPcb: (options) => {
    const w = options?.width ?? 50;
    const h = options?.height ?? 30;
    const layerCount = options?.layers ?? 2;
    const thickness = options?.thickness ?? 1.6;
    const traceWidth = options?.traceWidth ?? 0.15;
    const clearance = options?.clearance ?? 0.15;
    const boardName = options?.name ?? "PCB Board";

    const stackupLayers: Pcb["stackup"]["layers"] = [];
    stackupLayers.push({ layer: "FCu" as const, copperThickness: 0.035 });
    if (layerCount >= 4) {
      const innerCount = layerCount - 2;
      const innerLayerNames = ["In1Cu", "In2Cu", "In3Cu", "In4Cu"] as const;
      const dielectricPerLayer = thickness / (layerCount - 1);
      for (let i = 0; i < innerCount; i++) {
        stackupLayers.push({
          layer: innerLayerNames[i]!,
          copperThickness: 0.035,
          dielectricThickness: dielectricPerLayer,
          dielectricEr: 4.5,
          material: "FR4",
        });
      }
    }
    stackupLayers.push({
      layer: "BCu" as const,
      copperThickness: 0.035,
      dielectricThickness: layerCount >= 4 ? thickness / (layerCount - 1) : thickness,
      dielectricEr: 4.5,
      material: "FR4",
    });

    const pcbBoard: Pcb = {
      outline: {
        vertices: [
          { x: 0, y: 0 },
          { x: w, y: 0 },
          { x: w, y: h },
          { x: 0, y: h },
        ],
        thickness,
      },
      stackup: { layers: stackupLayers },
      nets: [],
      rules: {
        defaultRules: {
          name: "Default",
          traceWidth,
          clearance,
          viaDiameter: 0.6,
          viaDrill: 0.3,
        },
        edgeClearance: 0.25,
        holeToHole: 0.25,
        minAnnularRing: 0.13,
        minDrill: 0.2,
      },
      footprints: [],
      traces: [],
      vias: [],
      zones: [],
    };

    return addPcbToDocument(get, set, pcbBoard, boardName);
  },

  importPcb: (pcb, name) => {
    return addPcbToDocument(get, set, pcb, name ?? "Imported PCB");
  },

  syncSchematicToPcb: (boardNodeId) => {
    const state = get();
    const engine = state._crdtEngine!;

    const pcb = state.document.pcb ? structuredClone(state.document.pcb) : null;
    const schematic = state.document.schematic;
    if (!pcb || !schematic) return;

    const existingRefs = new Set(pcb.footprints.map((fp) => fp.ref));
    let added = 0;
    for (const comp of schematic.components) {
      if (existingRefs.has(comp.ref)) continue;
      let pads: Footprint["pads"] = [];
      let graphics: Footprint["graphics"] = [];
      if (comp.properties?.footprintTemplate) {
        try {
          const template = JSON.parse(comp.properties.footprintTemplate);
          pads = template.pads ?? [];
          graphics = template.graphics ?? [];
        } catch { /* skip */ }
      }
      const fpCount = pcb.footprints.length;
      const staggerX = 10 + ((fpCount + added) % 5) * 10;
      const staggerY = 10 + Math.floor((fpCount + added) / 5) * 10;
      pcb.footprints.push({
        ref: comp.ref, value: comp.value,
        footprintName: comp.footprintId ?? comp.ref,
        position: { x: staggerX, y: staggerY }, pads, graphics,
      });
      added++;
    }
    if (added === 0) return;
    const patch = setCrdtPcb(state, pcb);
    set({ ...patch, isDirty: true });
  },

  moveSchematicComponent: (idx, position) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    if (sch.components[idx]) {
      sch.components[idx]!.position = { x: position.x, y: position.y };
    }
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  moveSchematicComponentWithWires: (idx, position, wireUpdates) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    if (sch.components[idx]) {
      sch.components[idx]!.position = { x: position.x, y: position.y };
    }
    for (const wu of wireUpdates) {
      const wire = sch.wires[wu.wireIdx];
      if (wire) {
        wire[wu.endpoint] = { x: wu.pos.x, y: wu.pos.y };
      }
    }
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  moveFootprint: (_nodeId, idx, position) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    if (pcb.footprints[idx]) pcb.footprints[idx]!.position = { x: position.x, y: position.y };
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  rotateFootprint: (_nodeId, idx, angleDeg) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    if (pcb.footprints[idx]) {
      const fp = pcb.footprints[idx]!;
      fp.rotation = ((fp.rotation ?? 0) + angleDeg) % 360;
    }
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  flipFootprint: (_nodeId, idx) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    if (pcb.footprints[idx]) {
      const fp = pcb.footprints[idx]!;
      fp.front = !(fp.front ?? true);
    }
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  addTrace: (_nodeId, trace) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    pcb.traces.push({
      start: { x: trace.start.x, y: trace.start.y },
      end: { x: trace.end.x, y: trace.end.y },
      width: trace.width, layer: trace.layer as Pcb["traces"][number]["layer"], net: trace.net,
    });
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  removeTrace: (_nodeId, idx) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    pcb.traces.splice(idx, 1);
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  addVia: (_nodeId, via) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    pcb.vias.push({
      position: { x: via.position.x, y: via.position.y },
      diameter: via.diameter, drill: via.drill,
      startLayer: via.startLayer as Pcb["vias"][number]["startLayer"],
      endLayer: via.endLayer as Pcb["vias"][number]["endLayer"],
      net: via.net,
    });
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  removeVia: (_nodeId, idx) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    pcb.vias.splice(idx, 1);
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  addSchematicComponent: (comp, _boardNodeId) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    sch.components.push(structuredClone(comp));
    setCrdtSchematic(state, sch);
    // Auto-add footprint to PCB
    if (state.document.pcb && comp.footprintId && comp.properties?.footprintTemplate) {
      const pcb = structuredClone(state.document.pcb);
      try {
        const template = JSON.parse(comp.properties.footprintTemplate);
        const fpCount = pcb.footprints.length;
        pcb.footprints.push({
          ref: comp.ref, value: comp.value, footprintName: comp.footprintId,
          position: { x: 10 + (fpCount % 5) * 10, y: 10 + Math.floor(fpCount / 5) * 10 },
          pads: template.pads ?? [], graphics: template.graphics ?? [],
        });
      } catch { /* skip */ }
      set({ ...setCrdtPcb(state, pcb), isDirty: true });
      return;
    }
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeSchematicComponent: (idx, _boardNodeId) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    const removed = sch.components.splice(idx, 1);
    setCrdtSchematic(state, sch);
    if (state.document.pcb && removed[0]) {
      const pcb = structuredClone(state.document.pcb);
      pcb.footprints = pcb.footprints.filter((fp) => fp.ref !== removed[0]!.ref);
      set({ ...setCrdtPcb(state, pcb), isDirty: true });
      return;
    }
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  updateSchematicComponent: (idx, updates, _boardNodeId) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    if (sch.components[idx]) {
      Object.assign(sch.components[idx]!, updates);
    }
    setCrdtSchematic(state, sch);
    if (updates.value !== undefined && state.document.pcb) {
      const pcb = structuredClone(state.document.pcb);
      const ref = sch.components[idx]?.ref;
      if (ref) {
        const fp = pcb.footprints.find((f) => f.ref === ref);
        if (fp) fp.value = updates.value;
      }
      set({ ...setCrdtPcb(state, pcb), isDirty: true });
      return;
    }
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addSchematicWire: (wire) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    sch.wires.push({ start: { ...wire.start }, end: { ...wire.end } });
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeSchematicWire: (idx) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    sch.wires.splice(idx, 1);
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addSchematicLabel: (label) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    sch.labels.push(structuredClone(label));
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  removeSchematicLabel: (idx) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    sch.labels.splice(idx, 1);
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addSchematicJunction: (junction) => {
    const state = get();
    if (!state.document.schematic) return;
    const sch = structuredClone(state.document.schematic);
    sch.junctions.push({ position: { ...junction.position } });
    setCrdtSchematic(state, sch);
    const doc: Document = JSON.parse(state._crdtEngine!.get_document_json());
    set({ document: doc, isDirty: true });
  },

  addFootprint: (_nodeId, fp) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    pcb.footprints.push(structuredClone(fp));
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },

  removeFootprint: (_nodeId, idx) => {
    const state = get();
    if (!state.document.pcb) return;
    const pcb = structuredClone(state.document.pcb);
    pcb.footprints.splice(idx, 1);
    set({ ...setCrdtPcb(state, pcb), isDirty: true });
  },
}));
