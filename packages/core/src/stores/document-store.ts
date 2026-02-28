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

/** Result from a WasmDocumentEngine mutation */
interface CrdtMutationResult {
  document: Document;
  parts: PartInfo[];
  createdFeatureId?: string;
}

/** Minimal interface for WasmDocumentEngine (matches WASM exports) */
export interface WasmDocumentEngine {
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
 * Serialized as JSON for `set_param` / `create_feature`.
 */
type CrdtValue =
  | { F64: number }
  | { Vec3: [number, number, number] }
  | { Bool: boolean }
  | { String: string }
  | { FeatureRef: string }
  | { FeatureRefList: string[] }
  | { Sketch: string };

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
function crdtSketch(segments: SketchSegment2D[], plane: SketchPlane, origin: Vec3): CrdtValue {
  const { x_dir, y_dir } = getSketchPlaneDirections(plane);
  return { Sketch: JSON.stringify({ type: "Sketch2D", origin, x_dir, y_dir, segments }) };
}

export interface VcadFile {
  document: Document;
  parts: PartInfo[];
  consumedParts?: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum: number;
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
  nextPartNum: number;
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
  /** Map from legacy part IDs ("part-3") to CRDT feature IDs ("123:0"). */
  _partIdToCrdtId: Map<string, string>;
  /** Map from CRDT feature IDs ("123:0") to legacy part IDs ("part-3"). */
  _crdtIdToPartId: Map<string, string>;
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

// ---------------------------------------------------------------------------
// CRDT bridge helpers (module-level, not in store)
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
  return applyCrdtResult(result, state);
}

/** Shared logic for adding a Pcb board to the document. */
function addPcbToDocument(
  get: () => DocumentState,
  set: (s: Partial<DocumentState>) => void,
  pcbBoard: Pcb,
  boardName: string,
): NodeId {
  const state = get();
  const engine = state._crdtEngine;
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
  const patch = applyCrdtResult(result, state, result.createdFeatureId);
  set({ ...patch, isDirty: true });
  const pcbPart = (patch.parts ?? []).find((p) => p.kind === "pcb-board");
  return pcbPart ? (pcbPart as PcbBoardPartInfo).boardNodeId : 0;
}

/**
 * Apply a CRDT mutation result to the store, rewriting CRDT IDs to stable
 * legacy format and computing consumedParts.
 */
function applyCrdtResult(
  result: CrdtMutationResult,
  state: DocumentState,
  createdFeatureId?: string,
): Partial<DocumentState> {
  const parts = result.parts;
  const crdtIdToPartId = new Map(state._crdtIdToPartId);
  const partIdToCrdtId = new Map(state._partIdToCrdtId);
  let nextPartNum = state.nextPartNum;

  // Assign stable legacy IDs to CRDT parts
  for (const part of parts) {
    const crdtId = part.id;
    if (!crdtIdToPartId.has(crdtId)) {
      const legacyId = `part-${nextPartNum}`;
      crdtIdToPartId.set(crdtId, legacyId);
      partIdToCrdtId.set(legacyId, crdtId);
      nextPartNum++;
    }
  }

  // Rewrite part IDs from CRDT format to legacy format
  const rewrittenParts: PartInfo[] = parts.map((p) => {
    const legacyId = crdtIdToPartId.get(p.id) ?? p.id;
    const rewritten = { ...p, id: legacyId };

    // Rewrite source references in boolean/fillet/chamfer/shell/pattern parts
    if ("sourcePartIds" in rewritten && Array.isArray(rewritten.sourcePartIds)) {
      rewritten.sourcePartIds = (rewritten.sourcePartIds as string[]).map(
        (ref_id) => crdtIdToPartId.get(ref_id) ?? ref_id,
      ) as [string, string];
    }
    if ("sourcePartId" in rewritten && typeof rewritten.sourcePartId === "string") {
      (rewritten as Record<string, unknown>).sourcePartId =
        crdtIdToPartId.get(rewritten.sourcePartId as string) ?? rewritten.sourcePartId;
    }
    return rewritten;
  });

  // Compute consumedParts: any part referenced as input by another
  const consumedParts: Record<string, PartInfo> = {};
  const partMap = buildPartIndex(rewrittenParts);
  for (const part of rewrittenParts) {
    if ("sourcePartIds" in part && Array.isArray(part.sourcePartIds)) {
      for (const refId of part.sourcePartIds as string[]) {
        const consumed = partMap.get(refId);
        if (consumed) consumedParts[refId] = consumed;
      }
    }
    if ("sourcePartId" in part && typeof part.sourcePartId === "string") {
      const consumed = partMap.get(part.sourcePartId as string);
      if (consumed) consumedParts[part.sourcePartId as string] = consumed;
    }
  }

  // Compute which created legacy ID to return
  let createdLegacyId: string | undefined;
  if (createdFeatureId) {
    createdLegacyId = crdtIdToPartId.get(createdFeatureId);
  }

  // Find max node ID from the document
  let maxNodeId = 0;
  for (const nodeIdStr of Object.keys(result.document.nodes)) {
    const nid = Number(nodeIdStr);
    if (nid > maxNodeId) maxNodeId = nid;
  }

  return {
    document: result.document,
    parts: rewrittenParts,
    partIndex: buildPartIndex(rewrittenParts),
    consumedParts,
    nextNodeId: maxNodeId + 1,
    nextPartNum,
    isDirty: true,
    _crdtIdToPartId: crdtIdToPartId,
    _partIdToCrdtId: partIdToCrdtId,
    _lastCreatedPartId: createdLegacyId,
  } as Partial<DocumentState>;
}

/** Helper to get the created part ID from a patch */
function getCreatedPartId(patch: Partial<DocumentState>): string | undefined {
  return (patch as { _lastCreatedPartId?: string })._lastCreatedPartId;
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
  nextPartNum: 1,
  isDirty: false,
  documentId: null,
  documentName: "Untitled",
  lastSavedAt: null,
  isParameterDragging: false,
  loonSource: null,

  // CRDT bridge state
  _crdtEngine: null,
  _crdtEngineClass: null,
  _partIdToCrdtId: new Map(),
  _crdtIdToPartId: new Map(),

  _initCrdt: (EngineClass) => {
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
    const result: CrdtMutationResult = { document: doc, parts };
    const patch = applyCrdtResult(result, {
      ...get(),
      _crdtEngine: engine,
      _partIdToCrdtId: new Map(),
      _crdtIdToPartId: new Map(),
      nextPartNum: 1,
    });
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
    const state = get();
    const engine = state._crdtEngine!;
    const defaults: Record<PrimitiveKind, Record<string, CrdtValue>> = {
      cube: { size_x: crdtF64(20), size_y: crdtF64(20), size_z: crdtF64(20) },
      cylinder: { radius: crdtF64(10), height: crdtF64(20), segments: crdtF64(32) },
      sphere: { radius: crdtF64(10), segments: crdtF64(32) },
    };
    const result = engine.create_feature(kind, JSON.stringify(defaults[kind] ?? {}));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? `part-${state.nextPartNum}`;
  },

  removePart: (partId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.delete_feature(crdtId);
      set(applyCrdtResult(result, state));
    }
  },

  setTranslation: (partId, offset) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "offset", JSON.stringify(crdtVec3(offset)));
      set(applyCrdtResult(result, state));
    }
  },

  setRotation: (partId, angles) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "rotation", JSON.stringify(crdtVec3(angles)));
      set(applyCrdtResult(result, state));
    }
  },

  setScale: (partId, factor) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "scale", JSON.stringify(crdtVec3(factor)));
      set(applyCrdtResult(result, state));
    }
  },

  updatePrimitiveOp: (partId, op) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return;

    const o = op as Record<string, unknown>;
    let lastResult: CrdtMutationResult | undefined;
    if (o.type === "Cube" && "size" in o) {
      const size = o.size as Vec3;
      engine.set_param(crdtId, "size_x", JSON.stringify(crdtF64(size.x)));
      engine.set_param(crdtId, "size_y", JSON.stringify(crdtF64(size.y)));
      lastResult = engine.set_param(crdtId, "size_z", JSON.stringify(crdtF64(size.z)));
    } else if (o.type === "Cylinder" && "radius" in o) {
      engine.set_param(crdtId, "radius", JSON.stringify(crdtF64(o.radius as number)));
      engine.set_param(crdtId, "height", JSON.stringify(crdtF64(o.height as number)));
      lastResult = engine.set_param(crdtId, "segments", JSON.stringify(crdtF64(o.segments as number)));
    } else if (o.type === "Sphere" && "radius" in o) {
      engine.set_param(crdtId, "radius", JSON.stringify(crdtF64(o.radius as number)));
      lastResult = engine.set_param(crdtId, "segments", JSON.stringify(crdtF64(o.segments as number)));
    } else if (o.type === "Cone" && "radius_bottom" in o) {
      engine.set_param(crdtId, "radius_bottom", JSON.stringify(crdtF64(o.radius_bottom as number)));
      engine.set_param(crdtId, "radius_top", JSON.stringify(crdtF64(o.radius_top as number)));
      engine.set_param(crdtId, "height", JSON.stringify(crdtF64(o.height as number)));
      lastResult = engine.set_param(crdtId, "segments", JSON.stringify(crdtF64(o.segments as number)));
    }
    if (lastResult) {
      set(applyCrdtResult(lastResult, state));
    }
  },

  updateSweepOp: (partId, updates) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return;

    let lastResult: CrdtMutationResult | undefined;
    if (updates.twist_angle !== undefined) {
      lastResult = engine.set_param(crdtId, "twist_angle", JSON.stringify(crdtF64(updates.twist_angle)));
    }
    if (updates.scale_start !== undefined) {
      lastResult = engine.set_param(crdtId, "scale_start", JSON.stringify(crdtF64(updates.scale_start)));
    }
    if (updates.scale_end !== undefined) {
      lastResult = engine.set_param(crdtId, "scale_end", JSON.stringify(crdtF64(updates.scale_end)));
    }
    if (updates.path !== undefined) {
      lastResult = engine.set_param(crdtId, "path", JSON.stringify(crdtStr(JSON.stringify(updates.path))));
    }
    if (updates.orientation !== undefined) {
      lastResult = engine.set_param(crdtId, "orientation", JSON.stringify(crdtF64(updates.orientation)));
    }
    if (updates.path_segments !== undefined) {
      lastResult = engine.set_param(crdtId, "path_segments", JSON.stringify(crdtF64(updates.path_segments)));
    }
    if (updates.arc_segments !== undefined) {
      lastResult = engine.set_param(crdtId, "arc_segments", JSON.stringify(crdtF64(updates.arc_segments)));
    }
    if (lastResult) {
      set(applyCrdtResult(lastResult, state));
    }
  },

  setFeatureParam: (partId, key, value) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, key, JSON.stringify(value));
      set(applyCrdtResult(result, state));
    }
  },

  updateBooleanType: (partId, newType) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "boolean_type", JSON.stringify(crdtStr(newType)));
      set(applyCrdtResult(result, state));
    }
  },

  applyBoolean: (type, partIdA, partIdB) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtIdA = state._partIdToCrdtId.get(partIdA);
    const crdtIdB = state._partIdToCrdtId.get(partIdB);
    if (!crdtIdA || !crdtIdB) return null;

    const params: Record<string, CrdtValue> = {
      boolean_type: crdtStr(type),
      input_a: crdtRef(crdtIdA),
      input_b: crdtRef(crdtIdB),
    };
    const result = engine.create_feature("boolean", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  duplicateParts: (partIds) => {
    const state = get();
    const engine = state._crdtEngine!;
    const featuresJson = engine.get_ordered_features_json();
    const features: { id: string; kind: string; params: Record<string, unknown> }[] = JSON.parse(featuresJson);
    const newIds: string[] = [];
    let currentState = state;

    for (const partId of partIds) {
      const crdtId = currentState._partIdToCrdtId.get(partId);
      if (!crdtId) continue;

      const feature = features.find((f) => f.id === crdtId);
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

      const result = engine.create_feature(feature.kind, JSON.stringify(params));
      const patch = applyCrdtResult(result, currentState, result.createdFeatureId);
      currentState = { ...currentState, ...patch } as DocumentState;

      const createdId = getCreatedPartId(patch);
      if (createdId) newIds.push(createdId);
    }

    set(currentState);
    return newIds;
  },

  loadDocument: (file) => {
    const state = get();

    if (state._crdtEngineClass) {
      try {
        const irJson = JSON.stringify(file.document);
        const engine = state._crdtEngineClass.from_v1_json(irJson);
        const doc: Document = JSON.parse(engine.get_document_json());
        const parts: PartInfo[] = JSON.parse(engine.get_parts_json());
        const result: CrdtMutationResult = { document: doc, parts };
        const patch = applyCrdtResult(result, {
          ...state,
          _crdtEngine: engine,
          _partIdToCrdtId: new Map(),
          _crdtIdToPartId: new Map(),
          nextPartNum: 1,
        });
        if (state._crdtEngine) {
          state._crdtEngine.free();
        }
        set({
          ...patch,
          _crdtEngine: engine,
          isDirty: false,
          loonSource: file.loonSource ?? null,
        });
        return;
      } catch (e) {
        console.error("Failed to migrate legacy document to CRDT:", e);
      }
    }
  },

  addFromIR: (generatedDoc, name) => {
    const state = get();
    const engine = state._crdtEngine!;

    if (name && generatedDoc.roots.length > 0) {
      const firstRoot = generatedDoc.roots[0]!;
      const rootNode = generatedDoc.nodes[String(firstRoot.root)];
      if (rootNode) {
        rootNode.name = name;
      }
    }
    const irJson = JSON.stringify(generatedDoc);
    const result = engine.import_ir(irJson);
    const patch = applyCrdtResult(result, state);
    set({ ...patch, isDirty: true });
    return getCreatedPartId(patch) ?? null;
  },

  addExtrude: (plane, origin, segments, direction, options) => {
    if (segments.length === 0) return null;
    const state = get();
    const engine = state._crdtEngine!;

    const depth = Math.sqrt(direction.x ** 2 + direction.y ** 2 + direction.z ** 2);
    const dir = depth > 0 ? { x: direction.x / depth, y: direction.y / depth, z: direction.z / depth } : { x: 0, y: 0, z: 1 };
    const params: Record<string, CrdtValue> = {
      sketch: crdtSketch(segments, plane, origin),
      depth: crdtF64(depth),
      direction: crdtVec3(dir),
    };
    if (options?.twist_angle != null) params.twist_angle = crdtF64(options.twist_angle);
    if (options?.scale_end != null) params.scale_end = crdtF64(options.scale_end);
    const result = engine.create_feature("extrude", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addRevolve: (plane, origin, segments, axisOrigin, axisDir, angleDeg) => {
    if (segments.length === 0) return null;
    const state = get();
    const engine = state._crdtEngine!;

    const params: Record<string, CrdtValue> = {
      sketch: crdtSketch(segments, plane, origin),
      axis_origin: crdtVec3(axisOrigin),
      axis_dir: crdtVec3(axisDir),
      angle_deg: crdtF64(angleDeg),
    };
    const result = engine.create_feature("revolve", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addSweep: (plane, origin, segments, path, options = {}) => {
    if (segments.length === 0) return null;
    const state = get();
    const engine = state._crdtEngine!;

    const params: Record<string, CrdtValue> = {
      sketch: crdtSketch(segments, plane, origin),
      path: crdtStr(JSON.stringify(path)),
    };
    if (options.twist_angle != null) params.twist_angle = crdtF64(options.twist_angle);
    if (options.scale_start != null) params.scale_start = crdtF64(options.scale_start);
    if (options.scale_end != null) params.scale_end = crdtF64(options.scale_end);
    const result = engine.create_feature("sweep", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addLoft: (profiles, options = {}) => {
    if (profiles.length < 2) return null;
    const state = get();
    const engine = state._crdtEngine!;

    const params: Record<string, CrdtValue> = {
      sketch_count: crdtF64(profiles.length),
    };
    for (let i = 0; i < profiles.length; i++) {
      const p = profiles[i]!;
      params[`sketch_${i}`] = crdtSketch(p.segments, p.plane, p.origin);
    }
    if (options.closed != null) params.closed = crdtBool(options.closed);
    const result = engine.create_feature("loft", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addImportedMesh: (positions, indices, normals, source) => {
    const state = get();
    const engine = state._crdtEngine!;

    const params: Record<string, CrdtValue> = {
      positions_json: crdtStr(JSON.stringify(Array.from(positions))),
      indices_json: crdtStr(JSON.stringify(Array.from(indices))),
    };
    if (normals) params.normals_json = crdtStr(JSON.stringify(Array.from(normals)));
    if (source) params.source = crdtStr(source);
    const result = engine.create_feature("imported-mesh", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? `part-${state.nextPartNum}`;
  },

  addEmbroideryPattern: (design, source) => {
    const state = get();
    const engine = state._crdtEngine!;

    const filename = source?.split(/[/\\]/).pop()?.replace(/\.(pes|dst)$/i, "") ?? "Embroidery";
    const params: Record<string, CrdtValue> = {
      design: crdtStr(JSON.stringify(design)),
      name: crdtStr(filename),
    };
    if (source) params.source = crdtStr(source);
    const result = engine.create_feature("embroidery-pattern", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? `part-${state.nextPartNum}`;
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
    const crdtId = state._partIdToCrdtId.get(part.id);
    if (!crdtId) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.threads[threadIdx]) d.threads[threadIdx]!.color = color;
    const result = engine.set_param(crdtId, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyCrdtResult(result, state));
  },

  setThreadName: (nodeId, threadIdx, name) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const crdtId = state._partIdToCrdtId.get(part.id);
    if (!crdtId) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.threads[threadIdx]) d.threads[threadIdx]!.name = name;
    const result = engine.set_param(crdtId, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyCrdtResult(result, state));
  },

  setStitchGroupFillParams: (nodeId, groupIdx, params) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const crdtId = state._partIdToCrdtId.get(part.id);
    if (!crdtId) return;
    const design = getNodeEmbroideryDesign(state.document, nodeId);
    if (!design) return;
    const d = structuredClone(design);
    if (d.stitch_groups[groupIdx]) {
      const group = d.stitch_groups[groupIdx]!;
      group.fill_params = { ...(group.fill_params ?? DEFAULT_FILL_PARAMS), ...params };
    }
    const result = engine.set_param(crdtId, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyCrdtResult(result, state));
  },

  optimizeJumpStitches: (nodeId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.parts.find((p) => p.kind === "embroidery-pattern" && (p as EmbroideryPatternPartInfo).patternNodeId === nodeId);
    if (!part) return;
    const crdtId = state._partIdToCrdtId.get(part.id);
    if (!crdtId) return;
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
    const result = engine.set_param(crdtId, "design", JSON.stringify(crdtStr(JSON.stringify(d))));
    set(applyCrdtResult(result, state));
  },

  addFillet: (partId, radius) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;
    const params: Record<string, CrdtValue> = {
      input: crdtRef(crdtId),
      radius: crdtF64(radius),
    };
    const result = engine.create_feature("fillet", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addChamfer: (partId, distance) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;
    const params: Record<string, CrdtValue> = {
      input: crdtRef(crdtId),
      distance: crdtF64(distance),
    };
    const result = engine.create_feature("chamfer", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addShell: (partId, thickness) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;
    const params: Record<string, CrdtValue> = {
      input: crdtRef(crdtId),
      thickness: crdtF64(thickness),
    };
    const result = engine.create_feature("shell", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
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
      const crdtSourceId = state2._partIdToCrdtId.get(partId);
      const params: Record<string, CrdtValue> = {
        design: crdtStr(JSON.stringify(design)),
        name: crdtStr(`Stitch ${state2.nextPartNum}`),
      };
      if (crdtSourceId) params.input = crdtRef(crdtSourceId);
      const res = engine2.create_feature("embroidery-pattern", JSON.stringify(params));
      // Delete source feature (consumed)
      if (crdtSourceId) engine2.delete_feature(crdtSourceId);
      const patch = applyCrdtResult(res, state2, res.createdFeatureId);
      set({ ...patch, isDirty: true });
      return getCreatedPartId(patch) ?? null;
    } catch (err) {
      console.error("Failed to create stitch:", err);
      return null;
    }
  },

  addLinearPattern: (partId, direction, count, spacing) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;
    const params: Record<string, CrdtValue> = {
      input: crdtRef(crdtId),
      direction: crdtVec3(direction),
      count: crdtF64(count),
      spacing: crdtF64(spacing),
    };
    const result = engine.create_feature("linear-pattern", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addCircularPattern: (partId, axisOrigin, axisDir, count, angleDeg) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;
    const params: Record<string, CrdtValue> = {
      input: crdtRef(crdtId),
      axis_origin: crdtVec3(axisOrigin),
      axis_dir: crdtVec3(axisDir),
      count: crdtF64(count),
      angle_deg: crdtF64(angleDeg),
    };
    const result = engine.create_feature("circular-pattern", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addMirror: (partId, plane) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;
    const params: Record<string, CrdtValue> = {
      input: crdtRef(crdtId),
      plane: crdtStr(plane),
    };
    const result = engine.create_feature("mirror", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  addText: (options) => {
    const { text, height, depth, alignment, letterSpacing, lineSpacing } = options;
    if (!text.trim()) return null;

    const state = get();
    const engine = state._crdtEngine!;
    const params: Record<string, CrdtValue> = {
      text: crdtStr(text),
      height: crdtF64(height),
      depth: crdtF64(depth),
    };
    if (alignment) params.alignment = crdtStr(alignment);
    if (letterSpacing !== undefined) params.letter_spacing = crdtF64(letterSpacing);
    if (lineSpacing !== undefined) params.line_spacing = crdtF64(lineSpacing);
    const result = engine.create_feature("text", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return getCreatedPartId(patch) ?? null;
  },

  setPartMaterial: (partId, materialKey) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "material", JSON.stringify(crdtStr(materialKey)));
      set(applyCrdtResult(result, state));
    }
  },

  renamePart: (partId, name) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "name", JSON.stringify(crdtStr(name)));
      set(applyCrdtResult(result, state));
    }
  },

  undo: () => {
    const state = get();
    const engine = state._crdtEngine!;
    if (engine.can_undo()) {
      const result = engine.undo();
      set(applyCrdtResult(result, state));
    }
  },

  redo: () => {
    const state = get();
    const engine = state._crdtEngine!;
    if (engine.can_redo()) {
      const result = engine.redo();
      set(applyCrdtResult(result, state));
    }
  },

  markSaved: () => {
    set({ isDirty: false, lastSavedAt: Date.now() });
  },

  // Assembly operations
  setInstanceTransform: (instanceId, transform) => {
    const state = get();
    const engine = state._crdtEngine!;
    const result = engine.set_param(instanceId, "transform", JSON.stringify(crdtStr(JSON.stringify(transform))));
    set(applyCrdtResult(result, state));
  },

  setInstanceMaterial: (instanceId, materialKey) => {
    const state = get();
    const engine = state._crdtEngine!;
    const result = engine.set_param(instanceId, "material", JSON.stringify(crdtStr(materialKey)));
    set(applyCrdtResult(result, state));
  },

  setJointState: (jointId, jointState) => {
    const state = get();
    const engine = state._crdtEngine!;
    const result = engine.set_param(jointId, "state", JSON.stringify(crdtF64(jointState)));
    set(applyCrdtResult(result, state));
  },

  createPartDef: (partId, name) => {
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return null;

    const params: Record<string, CrdtValue> = {
      source_feature: crdtRef(crdtId),
    };
    if (name) params.name = crdtStr(name);
    const result = engine.create_feature("part-def", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return result.createdFeatureId ?? null;
  },

  createInstance: (partDefId, name, transform) => {
    const state = get();
    const engine = state._crdtEngine!;
    const params: Record<string, CrdtValue> = {
      part_def: crdtRef(partDefId),
    };
    if (name) params.name = crdtStr(name);
    if (transform) params.transform = crdtStr(JSON.stringify(transform));
    const result = engine.create_feature("instance", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return result.createdFeatureId ?? "";
  },

  addJoint: (config) => {
    const state = get();
    const engine = state._crdtEngine!;
    const params: Record<string, CrdtValue> = {
      kind: crdtStr(typeof config.kind === "string" ? config.kind : config.kind.type),
      child_instance: crdtRef(config.childInstanceId),
      anchor_a: crdtVec3(config.parentAnchor),
      anchor_b: crdtVec3(config.childAnchor),
    };
    if (config.parentInstanceId) params.parent_instance = crdtRef(config.parentInstanceId);
    if (config.name) params.name = crdtStr(config.name);
    // Extract axis from joint kind if present
    const jk = config.kind as Record<string, unknown>;
    if (jk.axis) params.axis = crdtVec3(jk.axis as Vec3);
    const result = engine.create_feature("joint", JSON.stringify(params));
    const patch = applyCrdtResult(result, state, result.createdFeatureId);
    set(patch);
    return result.createdFeatureId ?? "";
  },

  deleteInstance: (instanceId) => {
    const state = get();
    const engine = state._crdtEngine!;
    // Also delete any joints referencing this instance
    const featuresJson = engine.get_ordered_features_json();
    const features: { id: string; kind: string; params: Record<string, unknown> }[] = JSON.parse(featuresJson);
    for (const f of features) {
      if (f.kind === "joint") {
        const pi = f.params.parent_instance as { FeatureRef: string } | undefined;
        const ci = f.params.child_instance as { FeatureRef: string } | undefined;
        if ((pi && pi.FeatureRef === instanceId) || (ci && ci.FeatureRef === instanceId)) {
          engine.delete_feature(f.id);
        }
      }
    }
    const result = engine.delete_feature(instanceId);
    set(applyCrdtResult(result, state));
  },

  deleteJoint: (jointId) => {
    const state = get();
    const engine = state._crdtEngine!;
    const result = engine.delete_feature(jointId);
    set(applyCrdtResult(result, state));
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
    set(applyCrdtResult(result, state));
  },

  renameInstance: (instanceId, name) => {
    const state = get();
    const engine = state._crdtEngine!;
    const result = engine.set_param(instanceId, "name", JSON.stringify(crdtStr(name)));
    set(applyCrdtResult(result, state));
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
        nextPartNum: 1,
        isDirty: false,
        documentId: id,
        documentName: name,
        lastSavedAt: null,
        isParameterDragging: false,
        loonSource: null,
        _crdtEngine: engine,
        _partIdToCrdtId: new Map(),
        _crdtIdToPartId: new Map(),
      });
    } else {
      set({
        document: createDocument(),
        parts: [],
        partIndex: new Map(),
        consumedParts: {},
        nextNodeId: 1,
        nextPartNum: 1,
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
    const state = get();
    const engine = state._crdtEngine!;
    const crdtId = state._partIdToCrdtId.get(partId);
    if (crdtId) {
      const result = engine.set_param(crdtId, "visible", JSON.stringify(crdtBool(visible)));
      set(applyCrdtResult(result, state));
    }
  },

  reorderPart: (partId, newIndex) => {
    const state = get();
    const engine = state._crdtEngine!;
    const part = state.partIndex.get(partId);
    if (!part) return;

    const oldIndex = state.parts.findIndex((p) => p.id === partId);
    if (oldIndex === -1 || oldIndex === newIndex) return;

    const crdtId = state._partIdToCrdtId.get(partId);
    if (!crdtId) return;

    const featuresJson = engine.get_ordered_features_json();
    const features: { id: string }[] = JSON.parse(featuresJson);
    const others = features.filter((f) => f.id !== crdtId);

    const beforeId = newIndex > 0 ? others[newIndex - 1]?.id ?? "" : "";
    const afterId = newIndex < others.length ? others[newIndex]?.id ?? "" : "";

    const positionJson = engine.compute_position_between(beforeId, afterId);
    const result = engine.move_feature(crdtId, positionJson);
    set(applyCrdtResult(result, state));
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
    set(applyCrdtResult(result, state));
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
