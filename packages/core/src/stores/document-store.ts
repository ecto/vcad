import { create } from "zustand";
import type {
  Document,
  NodeId,
  CsgOp,
  Node,
  Vec3,
  SketchSegment2D,
  PathCurve,
  Transform3D,
  SweepOp,
  JointKind,
  Instance,
  Joint,
  PartDef,
  SceneSettings,
  Environment,
  Light,
  Background,
  PostProcessing,
  CameraPreset,
  TextAlignment,
} from "@vcad/ir";
import { createDocument, identityTransform } from "@vcad/ir";
import type {
  PartInfo,
  PrimitiveKind,
  BooleanType,
  BooleanPartInfo,
  ExtrudePartInfo,
  RevolvePartInfo,
  SweepPartInfo,
  LoftPartInfo,
  ImportedMeshPartInfo,
  FilletPartInfo,
  ChamferPartInfo,
  ShellPartInfo,
  LinearPatternPartInfo,
  CircularPatternPartInfo,
  MirrorPartInfo,
  TextPartInfo,
  SketchPlane,
} from "../types.js";
import {
  isPrimitivePart,
  isBooleanPart,
  isExtrudePart,
  isRevolvePart,
  isSweepPart,
  isLoftPart,
  isImportedMeshPart,
  isFilletPart,
  isChamferPart,
  isShellPart,
  isLinearPatternPart,
  isCircularPatternPart,
  isMirrorPart,
  isTextPart,
  getSketchPlaneDirections,
} from "../types.js";

const MAX_UNDO = 50;

interface Snapshot {
  document: string; // JSON-serialized Document
  parts: PartInfo[];
  consumedParts: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum: number;
  actionName: string; // Describes what action created this snapshot
  loonSource: string | null;
}

export interface VcadFile {
  document: Document;
  parts: PartInfo[];
  consumedParts?: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum: number;
  loonSource?: string | null;
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

  // Incremental evaluation tracking
  /** Node IDs that have changed since last evaluation */
  dirtyNodeIds: Set<NodeId>;
  /** Whether a parametric drag is in progress (enables LOD mode) */
  isParameterDragging: boolean;

  /** Loon source code — non-null when document was loaded from loon format. */
  loonSource: string | null;

  // undo/redo
  undoStack: Snapshot[];
  redoStack: Snapshot[];

  // mutations
  addPrimitive: (kind: PrimitiveKind) => string;
  removePart: (partId: string) => void;
  setTranslation: (partId: string, offset: Vec3, skipUndo?: boolean) => void;
  setRotation: (partId: string, angles: Vec3, skipUndo?: boolean) => void;
  setScale: (partId: string, factor: Vec3, skipUndo?: boolean) => void;
  updatePrimitiveOp: (partId: string, op: CsgOp, skipUndo?: boolean) => void;
  updateSweepOp: (
    partId: string,
    updates: Partial<SweepOp>,
    skipUndo?: boolean,
  ) => void;
  renamePart: (partId: string, name: string) => void;
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
  addText: (options: {
    text: string;
    height: number;
    depth: number;
    alignment?: TextAlignment;
    letterSpacing?: number;
    lineSpacing?: number;
  }) => string | null;
  // Incremental evaluation actions
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
}

function makeNode(id: NodeId, name: string | null, op: CsgOp): Node {
  return { id, name, op };
}

/** Build a Map index from parts array for O(1) lookups */
function buildPartIndex(parts: PartInfo[]): Map<string, PartInfo> {
  const index = new Map<string, PartInfo>();
  for (const part of parts) {
    index.set(part.id, part);
  }
  return index;
}

function snapshot(state: DocumentState, actionName: string): Snapshot {
  return {
    document: JSON.stringify(state.document),
    parts: state.parts.map((p) => ({ ...p })),
    consumedParts: { ...state.consumedParts },
    nextNodeId: state.nextNodeId,
    nextPartNum: state.nextPartNum,
    actionName,
    loonSource: state.loonSource,
  };
}

/** Add a node ID to the dirty set (for incremental evaluation) */
function markNodeDirty(state: DocumentState, nodeId: NodeId): Set<NodeId> {
  const newDirty = new Set(state.dirtyNodeIds);
  newDirty.add(nodeId);
  return newDirty;
}

function pushUndo(
  state: DocumentState,
  actionName: string,
): Pick<DocumentState, "undoStack" | "redoStack"> {
  const snap = snapshot(state, actionName);
  const stack = [...state.undoStack, snap];
  if (stack.length > MAX_UNDO) stack.shift();
  return { undoStack: stack, redoStack: [] };
}

// Selectors for undo/redo action names
export function getUndoActionName(state: DocumentState): string | null {
  const stack = state.undoStack;
  if (stack.length === 0) return null;
  return stack[stack.length - 1]!.actionName;
}

export function getRedoActionName(state: DocumentState): string | null {
  const stack = state.redoStack;
  if (stack.length === 0) return null;
  return stack[stack.length - 1]!.actionName;
}

const DEFAULT_SIZES: Record<PrimitiveKind, CsgOp> = {
  cube: { type: "Cube", size: { x: 20, y: 20, z: 20 } },
  cylinder: { type: "Cylinder", radius: 10, height: 20, segments: 32 },
  sphere: { type: "Sphere", radius: 10, segments: 32 },
};

const KIND_LABELS: Record<PrimitiveKind, string> = {
  cube: "Box",
  cylinder: "Cylinder",
  sphere: "Sphere",
};

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
  dirtyNodeIds: new Set<NodeId>(),
  isParameterDragging: false,
  loonSource: null,
  undoStack: [],
  redoStack: [],

  pushUndoSnapshot: () => {
    set((s) => pushUndo(s, "Edit"));
  },

  addPrimitive: (kind) => {
    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const primitiveId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const primOp = DEFAULT_SIZES[kind];
    const scaleOp: CsgOp = {
      type: "Scale",
      child: primitiveId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const partId = `part-${partNum}`;
    const name = `${KIND_LABELS[kind]} ${partNum}`;

    const newDoc = structuredClone(state.document);
    newDoc.nodes[String(primitiveId)] = makeNode(primitiveId, null, primOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(
      translateId,
      name,
      translateOp,
    );
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: PartInfo = {
      id: partId,
      name,
      kind,
      primitiveNodeId: primitiveId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const undoState = pushUndo(state, `Add ${KIND_LABELS[kind]}`);
    const newParts = [...state.parts, partInfo];

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  removePart: (partId) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = pushUndo(state, `Delete ${part.name}`);
    const newDoc = structuredClone(state.document);

    // Remove nodes based on part type
    if (isPrimitivePart(part)) {
      delete newDoc.nodes[String(part.primitiveNodeId)];
    } else if (isBooleanPart(part)) {
      delete newDoc.nodes[String(part.booleanNodeId)];
    } else if (isExtrudePart(part)) {
      delete newDoc.nodes[String(part.sketchNodeId)];
      delete newDoc.nodes[String(part.extrudeNodeId)];
    } else if (isRevolvePart(part)) {
      delete newDoc.nodes[String(part.sketchNodeId)];
      delete newDoc.nodes[String(part.revolveNodeId)];
    } else if (isSweepPart(part)) {
      delete newDoc.nodes[String(part.sketchNodeId)];
      delete newDoc.nodes[String(part.sweepNodeId)];
    } else if (isLoftPart(part)) {
      for (const sketchId of part.sketchNodeIds) {
        delete newDoc.nodes[String(sketchId)];
      }
      delete newDoc.nodes[String(part.loftNodeId)];
    } else if (isImportedMeshPart(part)) {
      delete newDoc.nodes[String(part.meshNodeId)];
    } else if (isFilletPart(part)) {
      delete newDoc.nodes[String(part.filletNodeId)];
    } else if (isChamferPart(part)) {
      delete newDoc.nodes[String(part.chamferNodeId)];
    } else if (isShellPart(part)) {
      delete newDoc.nodes[String(part.shellNodeId)];
    } else if (isLinearPatternPart(part)) {
      delete newDoc.nodes[String(part.patternNodeId)];
    } else if (isCircularPatternPart(part)) {
      delete newDoc.nodes[String(part.patternNodeId)];
    } else if (isMirrorPart(part)) {
      delete newDoc.nodes[String(part.mirrorNodeId)];
    } else if (isTextPart(part)) {
      delete newDoc.nodes[String(part.textNodeId)];
      delete newDoc.nodes[String(part.extrudeNodeId)];
    }
    delete newDoc.nodes[String(part.scaleNodeId)];
    delete newDoc.nodes[String(part.rotateNodeId)];
    delete newDoc.nodes[String(part.translateNodeId)];

    // Remove scene root
    newDoc.roots = newDoc.roots.filter((r) => r.root !== part.translateNodeId);
    const newParts = state.parts.filter((p) => p.id !== partId);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      isDirty: true,
      ...undoState,
    });
  },

  setTranslation: (partId, offset, skipUndo) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Transform");
    const nodeKey = String(part.translateNodeId);
    const oldNode = state.document.nodes[nodeKey];
    if (!oldNode || oldNode.op.type !== "Translate") return;

    // Shallow clone: only copy the modified node
    const newDoc: Document = {
      ...state.document,
      nodes: {
        ...state.document.nodes,
        [nodeKey]: { ...oldNode, op: { ...oldNode.op, offset } },
      },
    };

    set({
      document: newDoc,
      isDirty: true,
      dirtyNodeIds: markNodeDirty(state, part.translateNodeId),
      ...undoState,
    });
  },

  setRotation: (partId, angles, skipUndo) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Transform");
    const nodeKey = String(part.rotateNodeId);
    const oldNode = state.document.nodes[nodeKey];
    if (!oldNode || oldNode.op.type !== "Rotate") return;

    // Shallow clone: only copy the modified node
    const newDoc: Document = {
      ...state.document,
      nodes: {
        ...state.document.nodes,
        [nodeKey]: { ...oldNode, op: { ...oldNode.op, angles } },
      },
    };

    set({
      document: newDoc,
      isDirty: true,
      dirtyNodeIds: markNodeDirty(state, part.rotateNodeId),
      ...undoState,
    });
  },

  setScale: (partId, factor, skipUndo) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Transform");
    const nodeKey = String(part.scaleNodeId);
    const oldNode = state.document.nodes[nodeKey];
    if (!oldNode || oldNode.op.type !== "Scale") return;

    // Shallow clone: only copy the modified node
    const newDoc: Document = {
      ...state.document,
      nodes: {
        ...state.document.nodes,
        [nodeKey]: { ...oldNode, op: { ...oldNode.op, factor } },
      },
    };

    set({
      document: newDoc,
      isDirty: true,
      dirtyNodeIds: markNodeDirty(state, part.scaleNodeId),
      ...undoState,
    });
  },

  updatePrimitiveOp: (partId, op, skipUndo) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part || !isPrimitivePart(part)) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Edit Properties");
    const newDoc = structuredClone(state.document);
    const node = newDoc.nodes[String(part.primitiveNodeId)];
    if (node) {
      node.op = op;
    }

    set({
      document: newDoc,
      isDirty: true,
      dirtyNodeIds: markNodeDirty(state, part.primitiveNodeId),
      ...undoState,
    });
  },

  updateSweepOp: (partId, updates, skipUndo) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part || !isSweepPart(part)) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Edit Sweep");
    const newDoc = structuredClone(state.document);
    const node = newDoc.nodes[String(part.sweepNodeId)];
    if (node && node.op.type === "Sweep") {
      // Merge updates into the existing op
      node.op = { ...node.op, ...updates };
    }

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  applyBoolean: (type, partIdA, partIdB) => {
    const state = get();
    const partA = state.partIndex.get(partIdA);
    const partB = state.partIndex.get(partIdB);
    if (!partA || !partB) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const BOOL_OPS: Record<BooleanType, string> = {
      union: "Union",
      difference: "Difference",
      intersection: "Intersection",
    };

    const booleanId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const boolOp: CsgOp = {
      type: BOOL_OPS[type] as "Union" | "Difference" | "Intersection",
      left: partA.translateNodeId,
      right: partB.translateNodeId,
    } as CsgOp;

    const scaleOp: CsgOp = {
      type: "Scale",
      child: booleanId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const BOOL_LABELS: Record<BooleanType, string> = {
      union: "Union",
      difference: "Difference",
      intersection: "Intersection",
    };

    const partId = `part-${partNum}`;
    const name = `${BOOL_LABELS[type]} ${partNum}`;

    const undoState = pushUndo(state, BOOL_LABELS[type]);
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(booleanId)] = makeNode(booleanId, null, boolOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(
      translateId,
      name,
      translateOp,
    );

    // Remove source parts from roots (nodes stay for DAG references)
    newDoc.roots = newDoc.roots.filter(
      (r) =>
        r.root !== partA.translateNodeId && r.root !== partB.translateNodeId,
    );
    newDoc.roots.push({ root: translateId, material: "default" });

    const boolPartInfo: BooleanPartInfo = {
      id: partId,
      name,
      kind: "boolean",
      booleanType: type,
      booleanNodeId: booleanId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
      sourcePartIds: [partIdA, partIdB],
    };

    // Remove source parts from parts list, add boolean result
    // But preserve them in consumedParts for tree display
    const newConsumedParts = { ...state.consumedParts };
    newConsumedParts[partIdA] = partA;
    newConsumedParts[partIdB] = partB;

    const newParts = state.parts.filter(
      (p) => p.id !== partIdA && p.id !== partIdB,
    );
    newParts.push(boolPartInfo);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      consumedParts: newConsumedParts,
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  duplicateParts: (partIds) => {
    const state = get();
    const partsToClone = partIds
      .map((id) => state.partIndex.get(id))
      .filter((p): p is PartInfo => p !== undefined);
    if (partsToClone.length === 0) return [];

    let nid = state.nextNodeId;
    let pnum = state.nextPartNum;
    const undoState = pushUndo(state, "Duplicate");
    const newDoc = structuredClone(state.document);
    const newParts = [...state.parts];
    const newIds: string[] = [];

    for (const srcPart of partsToClone) {
      // Build a map of old node IDs to new node IDs for this part's subgraph
      const idMap = new Map<NodeId, NodeId>();

      // Collect all node IDs that belong to this part
      const nodeIdsToClone: NodeId[] = [];
      if (isPrimitivePart(srcPart)) {
        nodeIdsToClone.push(srcPart.primitiveNodeId);
      } else if (isBooleanPart(srcPart)) {
        // For boolean parts, we need the boolean node (the source nodes stay shared)
        nodeIdsToClone.push(srcPart.booleanNodeId);
      } else if (isExtrudePart(srcPart)) {
        nodeIdsToClone.push(srcPart.sketchNodeId, srcPart.extrudeNodeId);
      } else if (isRevolvePart(srcPart)) {
        nodeIdsToClone.push(srcPart.sketchNodeId, srcPart.revolveNodeId);
      } else if (isSweepPart(srcPart)) {
        nodeIdsToClone.push(srcPart.sketchNodeId, srcPart.sweepNodeId);
      } else if (isLoftPart(srcPart)) {
        nodeIdsToClone.push(...srcPart.sketchNodeIds, srcPart.loftNodeId);
      }
      nodeIdsToClone.push(
        srcPart.scaleNodeId,
        srcPart.rotateNodeId,
        srcPart.translateNodeId,
      );

      // Allocate new IDs
      for (const oldId of nodeIdsToClone) {
        idMap.set(oldId, nid++);
      }

      // Clone nodes with remapped IDs
      for (const oldId of nodeIdsToClone) {
        const oldNode = newDoc.nodes[String(oldId)];
        if (!oldNode) continue;

        const newId = idMap.get(oldId)!;
        const clonedOp = structuredClone(oldNode.op);

        // Remap child references
        if ("child" in clonedOp && typeof clonedOp.child === "number") {
          clonedOp.child = idMap.get(clonedOp.child) ?? clonedOp.child;
        }
        if ("left" in clonedOp && typeof clonedOp.left === "number") {
          clonedOp.left = idMap.get(clonedOp.left) ?? clonedOp.left;
        }
        if ("right" in clonedOp && typeof clonedOp.right === "number") {
          clonedOp.right = idMap.get(clonedOp.right) ?? clonedOp.right;
        }
        if ("sketch" in clonedOp && typeof clonedOp.sketch === "number") {
          clonedOp.sketch = idMap.get(clonedOp.sketch) ?? clonedOp.sketch;
        }
        if ("sketches" in clonedOp && Array.isArray(clonedOp.sketches)) {
          clonedOp.sketches = clonedOp.sketches.map(
            (id: number) => idMap.get(id) ?? id,
          );
        }

        newDoc.nodes[String(newId)] = makeNode(newId, oldNode.name, clonedOp);
      }

      // Offset the clone by +10mm on X
      const newTranslateId = idMap.get(srcPart.translateNodeId)!;
      const newTranslateNode = newDoc.nodes[String(newTranslateId)];
      if (newTranslateNode?.op.type === "Translate") {
        newTranslateNode.op.offset = {
          ...newTranslateNode.op.offset,
          x: newTranslateNode.op.offset.x + 10,
        };
      }

      // Add to roots
      newDoc.roots.push({ root: newTranslateId, material: "default" });

      // Build new PartInfo
      const partId = `part-${pnum}`;
      const partName = `${srcPart.name} copy`;

      if (newTranslateNode) newTranslateNode.name = partName;

      let clonedPartInfo: PartInfo;
      if (isPrimitivePart(srcPart)) {
        clonedPartInfo = {
          id: partId,
          name: partName,
          kind: srcPart.kind,
          primitiveNodeId: idMap.get(srcPart.primitiveNodeId)!,
          scaleNodeId: idMap.get(srcPart.scaleNodeId)!,
          rotateNodeId: idMap.get(srcPart.rotateNodeId)!,
          translateNodeId: newTranslateId,
        };
      } else if (isBooleanPart(srcPart)) {
        clonedPartInfo = {
          id: partId,
          name: partName,
          kind: "boolean",
          booleanType: srcPart.booleanType,
          booleanNodeId: idMap.get(srcPart.booleanNodeId)!,
          scaleNodeId: idMap.get(srcPart.scaleNodeId)!,
          rotateNodeId: idMap.get(srcPart.rotateNodeId)!,
          translateNodeId: newTranslateId,
          sourcePartIds: srcPart.sourcePartIds,
        };
      } else if (isExtrudePart(srcPart)) {
        clonedPartInfo = {
          id: partId,
          name: partName,
          kind: "extrude",
          sketchNodeId: idMap.get(srcPart.sketchNodeId)!,
          extrudeNodeId: idMap.get(srcPart.extrudeNodeId)!,
          scaleNodeId: idMap.get(srcPart.scaleNodeId)!,
          rotateNodeId: idMap.get(srcPart.rotateNodeId)!,
          translateNodeId: newTranslateId,
        };
      } else if (isRevolvePart(srcPart)) {
        clonedPartInfo = {
          id: partId,
          name: partName,
          kind: "revolve",
          sketchNodeId: idMap.get(srcPart.sketchNodeId)!,
          revolveNodeId: idMap.get(srcPart.revolveNodeId)!,
          scaleNodeId: idMap.get(srcPart.scaleNodeId)!,
          rotateNodeId: idMap.get(srcPart.rotateNodeId)!,
          translateNodeId: newTranslateId,
        };
      } else if (isSweepPart(srcPart)) {
        clonedPartInfo = {
          id: partId,
          name: partName,
          kind: "sweep",
          sketchNodeId: idMap.get(srcPart.sketchNodeId)!,
          sweepNodeId: idMap.get(srcPart.sweepNodeId)!,
          scaleNodeId: idMap.get(srcPart.scaleNodeId)!,
          rotateNodeId: idMap.get(srcPart.rotateNodeId)!,
          translateNodeId: newTranslateId,
        };
      } else {
        // isLoftPart
        const loftSrc = srcPart as LoftPartInfo;
        clonedPartInfo = {
          id: partId,
          name: partName,
          kind: "loft",
          sketchNodeIds: loftSrc.sketchNodeIds.map((id) => idMap.get(id)!),
          loftNodeId: idMap.get(loftSrc.loftNodeId)!,
          scaleNodeId: idMap.get(loftSrc.scaleNodeId)!,
          rotateNodeId: idMap.get(loftSrc.rotateNodeId)!,
          translateNodeId: newTranslateId,
        };
      }

      newParts.push(clonedPartInfo);
      newIds.push(partId);
      pnum++;
    }

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: pnum,
      isDirty: true,
      ...undoState,
    });

    return newIds;
  },

  loadDocument: (file) => {
    const parts = file.parts ?? [];
    set({
      document: file.document,
      parts,
      partIndex: buildPartIndex(parts),
      consumedParts: file.consumedParts ?? {},
      nextNodeId: file.nextNodeId ?? 1,
      nextPartNum: file.nextPartNum ?? 1,
      isDirty: false,
      dirtyNodeIds: new Set<NodeId>(),
      isParameterDragging: false,
      loonSource: file.loonSource ?? null,
      undoStack: [],
      redoStack: [],
    });
  },

  addFromIR: (generatedDoc, name) => {
    const state = get();

    // Build ID remapping: generated doc uses 0-based IDs, remap to current nextNodeId
    const idMap = new Map<NodeId, NodeId>();
    let nid = state.nextNodeId;

    // First pass: allocate new IDs for all nodes
    for (const oldId of Object.keys(generatedDoc.nodes)) {
      idMap.set(Number(oldId), nid++);
    }

    // Clone and remap all nodes
    const undoState = pushUndo(state, "AI Generate");
    const newDoc = structuredClone(state.document);

    for (const [oldIdStr, node] of Object.entries(generatedDoc.nodes)) {
      const oldId = Number(oldIdStr);
      const newId = idMap.get(oldId)!;
      const clonedOp = structuredClone(node.op);

      // Remap child references in the operation
      if ("child" in clonedOp && typeof clonedOp.child === "number") {
        clonedOp.child = idMap.get(clonedOp.child) ?? clonedOp.child;
      }
      if ("left" in clonedOp && typeof clonedOp.left === "number") {
        clonedOp.left = idMap.get(clonedOp.left) ?? clonedOp.left;
      }
      if ("right" in clonedOp && typeof clonedOp.right === "number") {
        clonedOp.right = idMap.get(clonedOp.right) ?? clonedOp.right;
      }
      if ("sketch" in clonedOp && typeof clonedOp.sketch === "number") {
        clonedOp.sketch = idMap.get(clonedOp.sketch) ?? clonedOp.sketch;
      }
      if ("sketches" in clonedOp && Array.isArray(clonedOp.sketches)) {
        clonedOp.sketches = clonedOp.sketches.map(
          (id: number) => idMap.get(id) ?? id
        );
      }

      newDoc.nodes[String(newId)] = makeNode(newId, node.name, clonedOp);
    }

    // Add roots from generated document, wrapping each with transform nodes
    const newParts = [...state.parts];
    let partNum = state.nextPartNum;
    let firstPartId: string | null = null;

    for (const rootEntry of generatedDoc.roots) {
      const remappedRootId = idMap.get(rootEntry.root);
      if (remappedRootId === undefined) continue;

      // Wrap the generated root with scale/rotate/translate nodes
      const scaleId = nid++;
      const rotateId = nid++;
      const translateId = nid++;

      const scaleOp: CsgOp = {
        type: "Scale",
        child: remappedRootId,
        factor: { x: 1, y: 1, z: 1 },
      };
      const rotateOp: CsgOp = {
        type: "Rotate",
        child: scaleId,
        angles: { x: 0, y: 0, z: 0 },
      };
      const translateOp: CsgOp = {
        type: "Translate",
        child: rotateId,
        offset: { x: 0, y: 0, z: 0 },
      };

      const partId = `part-${partNum}`;
      const partName = name ?? `AI Part ${partNum}`;

      newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
      newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
      newDoc.nodes[String(translateId)] = makeNode(translateId, partName, translateOp);

      // Add to document roots (the translateId is our new root)
      newDoc.roots.push({
        root: translateId,
        material: rootEntry.material ?? "default",
      });

      // Create PartInfo - use imported-mesh kind but with proper transform node IDs
      // This allows transform operations to work while keeping the AI-generated geometry
      const partInfo: ImportedMeshPartInfo = {
        id: partId,
        name: partName,
        kind: "imported-mesh",
        meshNodeId: remappedRootId,
        scaleNodeId: scaleId,
        rotateNodeId: rotateId,
        translateNodeId: translateId,
        source: "ai-generated",
      };

      newParts.push(partInfo);
      if (!firstPartId) firstPartId = partId;
      partNum++;
    }

    // Ensure default material exists
    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum,
      isDirty: true,
      ...undoState,
    });

    return firstPartId;
  },

  addExtrude: (plane, origin, segments, direction, options) => {
    if (segments.length === 0) return null;

    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const sketchId = nid++;
    const extrudeId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const { x_dir, y_dir } = getSketchPlaneDirections(plane);

    const sketchOp: CsgOp = {
      type: "Sketch2D",
      origin,
      x_dir,
      y_dir,
      segments,
    };

    const extrudeOp: CsgOp = {
      type: "Extrude",
      sketch: sketchId,
      direction,
      twist_angle: options?.twist_angle,
      scale_end: options?.scale_end,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: extrudeId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const partId = `part-${partNum}`;
    const name = `Extrude ${partNum}`;

    const undoState = pushUndo(state, "Extrude");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(sketchId)] = makeNode(sketchId, null, sketchOp);
    newDoc.nodes[String(extrudeId)] = makeNode(extrudeId, null, extrudeOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(
      translateId,
      name,
      translateOp,
    );
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: ExtrudePartInfo = {
      id: partId,
      name,
      kind: "extrude",
      sketchNodeId: sketchId,
      extrudeNodeId: extrudeId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newParts = [...state.parts, partInfo];
    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  addRevolve: (plane, origin, segments, axisOrigin, axisDir, angleDeg) => {
    if (segments.length === 0) return null;

    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const sketchId = nid++;
    const revolveId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const { x_dir, y_dir } = getSketchPlaneDirections(plane);

    const sketchOp: CsgOp = {
      type: "Sketch2D",
      origin,
      x_dir,
      y_dir,
      segments,
    };

    const revolveOp: CsgOp = {
      type: "Revolve",
      sketch: sketchId,
      axis_origin: axisOrigin,
      axis_dir: axisDir,
      angle_deg: angleDeg,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: revolveId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const partId = `part-${partNum}`;
    const name = `Revolve ${partNum}`;

    const undoState = pushUndo(state, "Revolve");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(sketchId)] = makeNode(sketchId, null, sketchOp);
    newDoc.nodes[String(revolveId)] = makeNode(revolveId, null, revolveOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(
      translateId,
      name,
      translateOp,
    );
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: RevolvePartInfo = {
      id: partId,
      name,
      kind: "revolve",
      sketchNodeId: sketchId,
      revolveNodeId: revolveId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newParts = [...state.parts, partInfo];
    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  addSweep: (plane, origin, segments, path, options = {}) => {
    if (segments.length === 0) return null;

    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const sketchId = nid++;
    const sweepId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const { x_dir, y_dir } = getSketchPlaneDirections(plane);

    const sketchOp: CsgOp = {
      type: "Sketch2D",
      origin,
      x_dir,
      y_dir,
      segments,
    };

    const sweepOp: CsgOp = {
      type: "Sweep",
      sketch: sketchId,
      path,
      twist_angle: options.twist_angle,
      scale_start: options.scale_start,
      scale_end: options.scale_end,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: sweepId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const partId = `part-${partNum}`;
    const name = `Sweep ${partNum}`;

    const undoState = pushUndo(state, "Sweep");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(sketchId)] = makeNode(sketchId, null, sketchOp);
    newDoc.nodes[String(sweepId)] = makeNode(sweepId, null, sweepOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(
      translateId,
      name,
      translateOp,
    );
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: SweepPartInfo = {
      id: partId,
      name,
      kind: "sweep",
      sketchNodeId: sketchId,
      sweepNodeId: sweepId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newParts = [...state.parts, partInfo];
    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  addLoft: (profiles, options = {}) => {
    if (profiles.length < 2) return null;

    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    // Create sketch nodes for each profile
    const sketchIds: number[] = [];
    const sketchOps: CsgOp[] = [];

    for (const profile of profiles) {
      const sketchId = nid++;
      sketchIds.push(sketchId);

      const { x_dir, y_dir } = getSketchPlaneDirections(profile.plane);
      sketchOps.push({
        type: "Sketch2D",
        origin: profile.origin,
        x_dir,
        y_dir,
        segments: profile.segments,
      });
    }

    const loftId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const loftOp: CsgOp = {
      type: "Loft",
      sketches: sketchIds,
      closed: options.closed,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: loftId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const partId = `part-${partNum}`;
    const name = `Loft ${partNum}`;

    const undoState = pushUndo(state, "Loft");
    const newDoc = structuredClone(state.document);

    // Add all sketch nodes
    for (let i = 0; i < sketchIds.length; i++) {
      newDoc.nodes[String(sketchIds[i])] = makeNode(
        sketchIds[i]!,
        null,
        sketchOps[i]!,
      );
    }

    newDoc.nodes[String(loftId)] = makeNode(loftId, null, loftOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(
      translateId,
      name,
      translateOp,
    );
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: LoftPartInfo = {
      id: partId,
      name,
      kind: "loft",
      sketchNodeIds: sketchIds,
      loftNodeId: loftId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newParts = [...state.parts, partInfo];
    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  addImportedMesh: (positions, indices, normals, source) => {
    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const meshId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const meshOp: CsgOp = {
      type: "ImportedMesh",
      positions: Array.from(positions),
      indices: Array.from(indices),
      normals: normals ? Array.from(normals) : undefined,
      source,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: meshId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    // Extract filename from source for display name
    const filename = source?.split(/[/\\]/).pop()?.replace(/\.(step|stp)$/i, "") ?? "Import";
    const partId = `part-${partNum}`;
    const name = `${filename} ${partNum}`;

    const undoState = pushUndo(state, "Import STEP");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(meshId)] = makeNode(meshId, null, meshOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: ImportedMeshPartInfo = {
      id: partId,
      name,
      kind: "imported-mesh",
      meshNodeId: meshId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
      source,
    };

    const newParts = [...state.parts, partInfo];
    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  addFillet: (partId, radius) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const filletId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const filletOp: CsgOp = {
      type: "Fillet",
      child: sourcePart.translateNodeId,
      radius,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: filletId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const newPartId = `part-${partNum}`;
    const name = `Fillet ${partNum}`;

    const undoState = pushUndo(state, "Fillet");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(filletId)] = makeNode(filletId, null, filletOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);

    // Remove source part from roots, add new part
    newDoc.roots = newDoc.roots.filter((r) => r.root !== sourcePart.translateNodeId);
    newDoc.roots.push({ root: translateId, material: "default" });

    const partInfo: FilletPartInfo = {
      id: newPartId,
      name,
      kind: "fillet",
      sourcePartId: partId,
      filletNodeId: filletId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    // Track source part as consumed
    const newConsumedParts = { ...state.consumedParts };
    newConsumedParts[partId] = sourcePart;

    const newParts = state.parts.filter((p) => p.id !== partId);
    newParts.push(partInfo);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      consumedParts: newConsumedParts,
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return newPartId;
  },

  addChamfer: (partId, distance) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const chamferId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const chamferOp: CsgOp = {
      type: "Chamfer",
      child: sourcePart.translateNodeId,
      distance,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: chamferId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const newPartId = `part-${partNum}`;
    const name = `Chamfer ${partNum}`;

    const undoState = pushUndo(state, "Chamfer");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(chamferId)] = makeNode(chamferId, null, chamferOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);

    newDoc.roots = newDoc.roots.filter((r) => r.root !== sourcePart.translateNodeId);
    newDoc.roots.push({ root: translateId, material: "default" });

    const partInfo: ChamferPartInfo = {
      id: newPartId,
      name,
      kind: "chamfer",
      sourcePartId: partId,
      chamferNodeId: chamferId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newConsumedParts = { ...state.consumedParts };
    newConsumedParts[partId] = sourcePart;

    const newParts = state.parts.filter((p) => p.id !== partId);
    newParts.push(partInfo);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      consumedParts: newConsumedParts,
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return newPartId;
  },

  addShell: (partId, thickness) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const shellId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const shellOp: CsgOp = {
      type: "Shell",
      child: sourcePart.translateNodeId,
      thickness,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: shellId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const newPartId = `part-${partNum}`;
    const name = `Shell ${partNum}`;

    const undoState = pushUndo(state, "Shell");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(shellId)] = makeNode(shellId, null, shellOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);

    newDoc.roots = newDoc.roots.filter((r) => r.root !== sourcePart.translateNodeId);
    newDoc.roots.push({ root: translateId, material: "default" });

    const partInfo: ShellPartInfo = {
      id: newPartId,
      name,
      kind: "shell",
      sourcePartId: partId,
      shellNodeId: shellId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newConsumedParts = { ...state.consumedParts };
    newConsumedParts[partId] = sourcePart;

    const newParts = state.parts.filter((p) => p.id !== partId);
    newParts.push(partInfo);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      consumedParts: newConsumedParts,
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return newPartId;
  },

  addLinearPattern: (partId, direction, count, spacing) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const patternId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const patternOp: CsgOp = {
      type: "LinearPattern",
      child: sourcePart.translateNodeId,
      direction,
      count,
      spacing,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: patternId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const newPartId = `part-${partNum}`;
    const name = `Linear Pattern ${partNum}`;

    const undoState = pushUndo(state, "Linear Pattern");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(patternId)] = makeNode(patternId, null, patternOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);

    newDoc.roots = newDoc.roots.filter((r) => r.root !== sourcePart.translateNodeId);
    newDoc.roots.push({ root: translateId, material: "default" });

    const partInfo: LinearPatternPartInfo = {
      id: newPartId,
      name,
      kind: "linear-pattern",
      sourcePartId: partId,
      patternNodeId: patternId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newConsumedParts = { ...state.consumedParts };
    newConsumedParts[partId] = sourcePart;

    const newParts = state.parts.filter((p) => p.id !== partId);
    newParts.push(partInfo);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      consumedParts: newConsumedParts,
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return newPartId;
  },

  addCircularPattern: (partId, axisOrigin, axisDir, count, angleDeg) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const patternId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    const patternOp: CsgOp = {
      type: "CircularPattern",
      child: sourcePart.translateNodeId,
      axis_origin: axisOrigin,
      axis_dir: axisDir,
      count,
      angle_deg: angleDeg,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: patternId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const newPartId = `part-${partNum}`;
    const name = `Circular Pattern ${partNum}`;

    const undoState = pushUndo(state, "Circular Pattern");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(patternId)] = makeNode(patternId, null, patternOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);

    newDoc.roots = newDoc.roots.filter((r) => r.root !== sourcePart.translateNodeId);
    newDoc.roots.push({ root: translateId, material: "default" });

    const partInfo: CircularPatternPartInfo = {
      id: newPartId,
      name,
      kind: "circular-pattern",
      sourcePartId: partId,
      patternNodeId: patternId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newConsumedParts = { ...state.consumedParts };
    newConsumedParts[partId] = sourcePart;

    const newParts = state.parts.filter((p) => p.id !== partId);
    newParts.push(partInfo);

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      consumedParts: newConsumedParts,
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return newPartId;
  },

  addMirror: (partId, plane) => {
    const state = get();
    const sourcePart = state.partIndex.get(partId);
    if (!sourcePart) return null;

    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    // Mirror is implemented as a Scale with negative factor on one axis
    const mirrorId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    // Determine scale factor based on mirror plane
    const mirrorFactor = {
      XY: { x: 1, y: 1, z: -1 },
      XZ: { x: 1, y: -1, z: 1 },
      YZ: { x: -1, y: 1, z: 1 },
    }[plane];

    const mirrorOp: CsgOp = {
      type: "Scale",
      child: sourcePart.translateNodeId,
      factor: mirrorFactor,
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: mirrorId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const newPartId = `part-${partNum}`;
    const name = `Mirror ${plane} ${partNum}`;

    const undoState = pushUndo(state, "Mirror");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(mirrorId)] = makeNode(mirrorId, null, mirrorOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);

    // Keep source part in roots, add mirror as additional part
    newDoc.roots.push({ root: translateId, material: "default" });

    const partInfo: MirrorPartInfo = {
      id: newPartId,
      name,
      kind: "mirror",
      sourcePartId: partId,
      mirrorNodeId: mirrorId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    // Mirror keeps source, so don't remove from parts
    const newParts = [...state.parts, partInfo];

    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return newPartId;
  },

  addText: (options) => {
    const { text, height, depth, alignment, letterSpacing, lineSpacing } = options;
    if (!text.trim()) return null;

    const state = get();
    let nid = state.nextNodeId;
    const partNum = state.nextPartNum;

    const textId = nid++;
    const extrudeId = nid++;
    const scaleId = nid++;
    const rotateId = nid++;
    const translateId = nid++;

    // Default to XY plane at origin
    const origin = { x: 0, y: 0, z: 0 };
    const x_dir = { x: 1, y: 0, z: 0 };
    const y_dir = { x: 0, y: 1, z: 0 };

    const textOp: CsgOp = {
      type: "Text2D",
      origin,
      x_dir,
      y_dir,
      text,
      font: "sans-serif",
      height,
      letter_spacing: letterSpacing,
      line_spacing: lineSpacing,
      alignment: alignment ?? "left",
    };

    const extrudeOp: CsgOp = {
      type: "Extrude",
      sketch: textId,
      direction: { x: 0, y: 0, z: depth },
    };

    const scaleOp: CsgOp = {
      type: "Scale",
      child: extrudeId,
      factor: { x: 1, y: 1, z: 1 },
    };
    const rotateOp: CsgOp = {
      type: "Rotate",
      child: scaleId,
      angles: { x: 0, y: 0, z: 0 },
    };
    const translateOp: CsgOp = {
      type: "Translate",
      child: rotateId,
      offset: { x: 0, y: 0, z: 0 },
    };

    const partId = `part-${partNum}`;
    // Use first few chars of text as name preview
    const preview = text.length > 10 ? text.slice(0, 10) + "…" : text;
    const name = `Text "${preview}"`;

    const undoState = pushUndo(state, "Add Text");
    const newDoc = structuredClone(state.document);

    newDoc.nodes[String(textId)] = makeNode(textId, null, textOp);
    newDoc.nodes[String(extrudeId)] = makeNode(extrudeId, null, extrudeOp);
    newDoc.nodes[String(scaleId)] = makeNode(scaleId, null, scaleOp);
    newDoc.nodes[String(rotateId)] = makeNode(rotateId, null, rotateOp);
    newDoc.nodes[String(translateId)] = makeNode(translateId, name, translateOp);
    newDoc.roots.push({ root: translateId, material: "default" });

    if (!newDoc.materials["default"]) {
      newDoc.materials["default"] = {
        name: "Default",
        color: [0.55, 0.55, 0.55],
        metallic: 0.0,
        roughness: 0.7,
      };
    }

    const partInfo: TextPartInfo = {
      id: partId,
      name,
      kind: "text",
      textNodeId: textId,
      extrudeNodeId: extrudeId,
      scaleNodeId: scaleId,
      rotateNodeId: rotateId,
      translateNodeId: translateId,
    };

    const newParts = [...state.parts, partInfo];
    set({
      document: newDoc,
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      nextNodeId: nid,
      nextPartNum: partNum + 1,
      isDirty: true,
      ...undoState,
    });

    return partId;
  },

  setPartMaterial: (partId, materialKey) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = pushUndo(state, "Set Material");
    const newDoc = structuredClone(state.document);

    // Update the root entry's material
    const rootEntry = newDoc.roots.find((r) => r.root === part.translateNodeId);
    if (rootEntry) {
      rootEntry.material = materialKey;
    }

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  renamePart: (partId, name) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = pushUndo(state, "Rename");
    const newParts = state.parts.map((p) =>
      p.id === partId ? { ...p, name } : p,
    );
    const newDoc = structuredClone(state.document);
    const node = newDoc.nodes[String(part.translateNodeId)];
    if (node) node.name = name;

    set({
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      document: newDoc,
      isDirty: true,
      ...undoState,
    });
  },

  undo: () => {
    const state = get();
    if (state.undoStack.length === 0) return;

    const prevSnap = state.undoStack[state.undoStack.length - 1]!;
    // Create snapshot with the action name we're undoing (for redo stack)
    const currentSnap = snapshot(state, prevSnap.actionName);

    set({
      document: JSON.parse(prevSnap.document) as Document,
      parts: prevSnap.parts,
      partIndex: buildPartIndex(prevSnap.parts),
      consumedParts: prevSnap.consumedParts,
      nextNodeId: prevSnap.nextNodeId,
      nextPartNum: prevSnap.nextPartNum,
      loonSource: prevSnap.loonSource,
      isDirty: true,
      undoStack: state.undoStack.slice(0, -1),
      redoStack: [...state.redoStack, currentSnap],
    });
  },

  redo: () => {
    const state = get();
    if (state.redoStack.length === 0) return;

    const nextSnap = state.redoStack[state.redoStack.length - 1]!;
    // Create snapshot with the action name we're redoing (for undo stack)
    const currentSnap = snapshot(state, nextSnap.actionName);

    set({
      document: JSON.parse(nextSnap.document) as Document,
      parts: nextSnap.parts,
      partIndex: buildPartIndex(nextSnap.parts),
      consumedParts: nextSnap.consumedParts,
      nextNodeId: nextSnap.nextNodeId,
      nextPartNum: nextSnap.nextPartNum,
      loonSource: nextSnap.loonSource,
      isDirty: true,
      undoStack: [...state.undoStack, currentSnap],
      redoStack: state.redoStack.slice(0, -1),
    });
  },

  markSaved: () => {
    set({ isDirty: false, lastSavedAt: Date.now() });
  },

  // Assembly operations
  setInstanceTransform: (instanceId, transform, skipUndo) => {
    const state = get();
    if (!state.document.instances) return;

    const idx = state.document.instances.findIndex((i) => i.id === instanceId);
    if (idx === -1) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Transform Instance");
    const newDoc = structuredClone(state.document);
    const instance = newDoc.instances![idx]!;
    newDoc.instances![idx] = { ...instance, transform };

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  setInstanceMaterial: (instanceId, materialKey) => {
    const state = get();
    if (!state.document.instances) return;

    const idx = state.document.instances.findIndex((i) => i.id === instanceId);
    if (idx === -1) return;

    const undoState = pushUndo(state, "Set Instance Material");
    const newDoc = structuredClone(state.document);
    const instance = newDoc.instances![idx]!;
    newDoc.instances![idx] = { ...instance, material: materialKey };

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  setJointState: (jointId, newState, skipUndo) => {
    const state = get();
    if (!state.document.joints) return;

    const idx = state.document.joints.findIndex((j) => j.id === jointId);
    if (idx === -1) return;

    // Early return if value unchanged (avoids creating new document reference)
    const currentJoint = state.document.joints[idx]!;
    if (currentJoint.state === newState) return;

    const undoState = skipUndo ? {} : pushUndo(state, "Adjust Joint");
    const newDoc = structuredClone(state.document);
    const joint = newDoc.joints![idx]!;
    newDoc.joints![idx] = { ...joint, state: newState };

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  createPartDef: (partId, name) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return null;

    const undoState = pushUndo(state, "Create Part Definition");
    const newDoc = structuredClone(state.document);

    // Initialize partDefs if needed
    if (!newDoc.partDefs) {
      newDoc.partDefs = {};
    }

    // Generate unique ID
    const existingCount = Object.keys(newDoc.partDefs).length;
    const partDefId = `partdef-${existingCount + 1}`;
    const defName = name ?? `${part.name} Def`;

    // Create part definition pointing to this part's root node
    const partDef: PartDef = {
      id: partDefId,
      name: defName,
      root: part.translateNodeId,
    };

    newDoc.partDefs[partDefId] = partDef;

    // Initialize instances array if needed
    if (!newDoc.instances) {
      newDoc.instances = [];
    }

    // Create first instance of this part definition
    const instanceId = `instance-${newDoc.instances.length + 1}`;
    const instance: Instance = {
      id: instanceId,
      partDefId,
      name: part.name,
      transform: identityTransform(),
    };
    newDoc.instances.push(instance);

    // Remove this part from roots (it's now managed via instances)
    newDoc.roots = newDoc.roots.filter((r) => r.root !== part.translateNodeId);

    // If this is the first instance, make it the ground
    if (newDoc.instances.length === 1) {
      newDoc.groundInstanceId = instanceId;
    }

    // Remove from parts list since it's now a partDef
    const newParts = state.parts.filter((p) => p.id !== partId);

    set({
      document: newDoc,
      parts: newParts,
      isDirty: true,
      ...undoState,
    });

    return partDefId;
  },

  createInstance: (partDefId, name, transform) => {
    const state = get();
    const undoState = pushUndo(state, "Insert Instance");
    const newDoc = structuredClone(state.document);

    if (!newDoc.instances) {
      newDoc.instances = [];
    }

    const partDef = newDoc.partDefs?.[partDefId];
    const instanceNum = newDoc.instances.length + 1;
    const instanceId = `instance-${instanceNum}`;

    // Default transform: offset slightly from origin
    const defaultTransform = transform ?? {
      translation: { x: instanceNum * 30, y: 0, z: 0 },
      rotation: { x: 0, y: 0, z: 0 },
      scale: { x: 1, y: 1, z: 1 },
    };

    const instance: Instance = {
      id: instanceId,
      partDefId,
      name: name ?? partDef?.name ?? `Instance ${instanceNum}`,
      transform: defaultTransform,
    };

    newDoc.instances.push(instance);

    set({ document: newDoc, isDirty: true, ...undoState });
    return instanceId;
  },

  addJoint: (config) => {
    const state = get();
    const undoState = pushUndo(state, "Add Joint");
    const newDoc = structuredClone(state.document);

    if (!newDoc.joints) {
      newDoc.joints = [];
    }

    const jointNum = newDoc.joints.length + 1;
    const jointId = `joint-${jointNum}`;

    const joint: Joint = {
      id: jointId,
      name: config.name,
      parentInstanceId: config.parentInstanceId,
      childInstanceId: config.childInstanceId,
      parentAnchor: config.parentAnchor,
      childAnchor: config.childAnchor,
      kind: config.kind,
      state: 0,
    };

    newDoc.joints.push(joint);

    set({ document: newDoc, isDirty: true, ...undoState });
    return jointId;
  },

  deleteInstance: (instanceId) => {
    const state = get();
    if (!state.document.instances) return;

    const instance = state.document.instances.find((i) => i.id === instanceId);
    if (!instance) return;

    const undoState = pushUndo(state, "Delete Instance");
    const newDoc = structuredClone(state.document);

    // Remove the instance
    newDoc.instances = newDoc.instances!.filter((i) => i.id !== instanceId);

    // Remove any joints that reference this instance
    if (newDoc.joints) {
      newDoc.joints = newDoc.joints.filter(
        (j) =>
          j.parentInstanceId !== instanceId && j.childInstanceId !== instanceId,
      );
    }

    // If this was the ground instance, clear it or assign to another
    if (newDoc.groundInstanceId === instanceId) {
      newDoc.groundInstanceId =
        newDoc.instances.length > 0 ? newDoc.instances[0]!.id : undefined;
    }

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  deleteJoint: (jointId) => {
    const state = get();
    if (!state.document.joints) return;

    const undoState = pushUndo(state, "Delete Joint");
    const newDoc = structuredClone(state.document);

    newDoc.joints = newDoc.joints!.filter((j) => j.id !== jointId);

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  setGroundInstance: (instanceId) => {
    const state = get();
    if (!state.document.instances) return;

    const exists = state.document.instances.some((i) => i.id === instanceId);
    if (!exists) return;

    const undoState = pushUndo(state, "Set Ground");
    const newDoc = structuredClone(state.document);
    newDoc.groundInstanceId = instanceId;

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  renameInstance: (instanceId, name) => {
    const state = get();
    if (!state.document.instances) return;

    const idx = state.document.instances.findIndex((i) => i.id === instanceId);
    if (idx === -1) return;

    const undoState = pushUndo(state, "Rename Instance");
    const newDoc = structuredClone(state.document);
    const instance = newDoc.instances![idx]!;
    newDoc.instances![idx] = { ...instance, name };

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  setDocumentMeta: (id, name) => {
    set({ documentId: id, documentName: name });
  },

  setDocumentName: (name) => {
    set({ documentName: name, isDirty: true });
  },

  newDocument: (id, name) => {
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
      dirtyNodeIds: new Set<NodeId>(),
      isParameterDragging: false,
      loonSource: null,
      undoStack: [],
      redoStack: [],
    });
  },

  clearDirtyNodes: () => {
    const state = get();
    const dirty = state.dirtyNodeIds;
    set({ dirtyNodeIds: new Set<NodeId>() });
    return dirty;
  },

  setParameterDragging: (dragging) => {
    set({ isParameterDragging: dragging });
  },

  setPartVisible: (partId, visible) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const undoState = pushUndo(state, visible ? "Show Part" : "Hide Part");
    const newDoc = structuredClone(state.document);

    // Find and update the root entry
    const rootIdx = newDoc.roots.findIndex((r) => r.root === part.translateNodeId);
    if (rootIdx !== -1) {
      const entry = newDoc.roots[rootIdx]!;
      newDoc.roots[rootIdx] = { ...entry, visible };
    }

    set({ document: newDoc, isDirty: true, ...undoState });
  },

  reorderPart: (partId, newIndex) => {
    const state = get();
    const part = state.partIndex.get(partId);
    if (!part) return;

    const oldIndex = state.parts.findIndex((p) => p.id === partId);
    if (oldIndex === -1 || oldIndex === newIndex) return;

    const undoState = pushUndo(state, "Reorder");

    // Reorder parts array
    const newParts = [...state.parts];
    const [removed] = newParts.splice(oldIndex, 1);
    if (removed) {
      newParts.splice(newIndex, 0, removed);
    }

    // Also reorder document.roots to keep in sync
    const newDoc = structuredClone(state.document);
    const rootIndex = newDoc.roots.findIndex((r) => r.root === part.translateNodeId);
    if (rootIndex !== -1) {
      const [rootEntry] = newDoc.roots.splice(rootIndex, 1);
      if (rootEntry) {
        // Find corresponding new position in roots
        const targetPart = newParts[newIndex];
        const targetRootIndex = targetPart
          ? newDoc.roots.findIndex((r) => r.root === targetPart.translateNodeId)
          : newDoc.roots.length;
        newDoc.roots.splice(
          targetRootIndex !== -1 ? targetRootIndex : newDoc.roots.length,
          0,
          rootEntry
        );
      }
    }

    set({
      parts: newParts,
      partIndex: buildPartIndex(newParts),
      document: newDoc,
      isDirty: true,
      ...undoState,
    });
  },

  // Scene settings actions
  setSceneSettings: (settings) => {
    const state = get();
    const undoState = pushUndo(state, "Update Scene Settings");
    const newDoc = structuredClone(state.document);
    newDoc.scene = settings;
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  updateEnvironment: (environment) => {
    const state = get();
    const undoState = pushUndo(state, "Update Environment");
    const newDoc = structuredClone(state.document);
    newDoc.scene = { ...newDoc.scene, environment };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  updateLights: (lights) => {
    const state = get();
    const undoState = pushUndo(state, "Update Lights");
    const newDoc = structuredClone(state.document);
    newDoc.scene = { ...newDoc.scene, lights };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  addLight: (light) => {
    const state = get();
    const undoState = pushUndo(state, "Add Light");
    const newDoc = structuredClone(state.document);
    const currentLights = newDoc.scene?.lights ?? [];
    newDoc.scene = { ...newDoc.scene, lights: [...currentLights, light] };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  removeLight: (lightId) => {
    const state = get();
    const undoState = pushUndo(state, "Remove Light");
    const newDoc = structuredClone(state.document);
    const currentLights = newDoc.scene?.lights ?? [];
    newDoc.scene = { ...newDoc.scene, lights: currentLights.filter((l) => l.id !== lightId) };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  updateLight: (lightId, updates) => {
    const state = get();
    const undoState = pushUndo(state, "Update Light");
    const newDoc = structuredClone(state.document);
    const currentLights = newDoc.scene?.lights ?? [];
    newDoc.scene = {
      ...newDoc.scene,
      lights: currentLights.map((l) => (l.id === lightId ? { ...l, ...updates } : l)),
    };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  updateBackground: (background) => {
    const state = get();
    const undoState = pushUndo(state, "Update Background");
    const newDoc = structuredClone(state.document);
    newDoc.scene = { ...newDoc.scene, background };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  updatePostProcessing: (postProcessing) => {
    const state = get();
    const undoState = pushUndo(state, "Update Post-Processing");
    const newDoc = structuredClone(state.document);
    newDoc.scene = { ...newDoc.scene, postProcessing };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  addCameraPreset: (preset) => {
    const state = get();
    const undoState = pushUndo(state, "Add Camera Preset");
    const newDoc = structuredClone(state.document);
    const currentPresets = newDoc.scene?.cameraPresets ?? [];
    newDoc.scene = { ...newDoc.scene, cameraPresets: [...currentPresets, preset] };
    set({ document: newDoc, isDirty: true, ...undoState });
  },

  removeCameraPreset: (presetId) => {
    const state = get();
    const undoState = pushUndo(state, "Remove Camera Preset");
    const newDoc = structuredClone(state.document);
    const currentPresets = newDoc.scene?.cameraPresets ?? [];
    newDoc.scene = { ...newDoc.scene, cameraPresets: currentPresets.filter((p) => p.id !== presetId) };
    set({ document: newDoc, isDirty: true, ...undoState });
  },
}));
