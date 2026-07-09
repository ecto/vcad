import type { Document, NodeId, CsgOp, Node } from "@vcad/ir";
import { fromVCode, createDocument } from "@vcad/ir";
import type { PartInfo, PrimitiveKind } from "../types.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let wasmModule: any = null;

async function loadWasm(): Promise<typeof wasmModule | null> {
  if (wasmModule) return wasmModule;
  try {
    wasmModule = await import("@vcad/kernel-wasm");
    return wasmModule;
  } catch {
    return null;
  }
}

// Eagerly start loading
loadWasm();

/**
 * A parsed `.vcad` file.
 *
 * CRDT is the canonical persistence format (v0.4). Legacy IR-JSON (v0.1) and
 * VCode (v0.2) formats are still accepted for one-shot import, and Loon
 * (v0.3) remains the declarative-authoring format. The tagged `kind` field
 * forces every consumer to discriminate — preventing the silent-drop bugs
 * that plagued the old undifferentiated shape (where CRDT params not
 * surfaced by the materializer would vanish on save/load).
 */
export type VcadFile =
  | VcadFileCrdt
  | VcadFileLoon
  | VcadFileLegacy;

/** v0.4 — CRDT canonical format. The preferred shape for every new save. */
export interface VcadFileCrdt {
  kind: "crdt";
  version: "0.4";
  /** Raw output of `WasmDocumentEngine::save()` — JSON-encoded CRDT state. */
  crdtBytes: Uint8Array;
}

/** v0.3 — Loon source text. The declarative-authoring format. */
export interface VcadFileLoon {
  kind: "loon";
  version: "0.3";
  loonSource: string;
  document: Document;
  parts: PartInfo[];
  nextNodeId: number;
  nextPartNum?: number;
}

/** v0.1 / v0.2 — legacy IR-JSON or VCode. Read-only; never written by new saves. */
export interface VcadFileLegacy {
  kind: "legacy";
  version: "0.1" | "0.2";
  document: Document;
  parts: PartInfo[];
  consumedParts?: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum?: number;
}

/**
 * Return an IR `Document` for read-only display surfaces (thumbnails, header,
 * search) when the caller doesn't have access to a live engine.
 *
 * For CRDT files there IS no pre-materialized document in the VcadFile — the
 * caller must bring their own engine to materialize. Returns `null` in that
 * case so UI surfaces can degrade gracefully (e.g. skip thumbnails until the
 * engine finishes booting) rather than silently render an empty scene.
 */
export function getDocumentForDisplay(file: VcadFile): Document | null {
  switch (file.kind) {
    case "crdt":
      return null;
    case "loon":
    case "legacy":
      return file.document;
  }
}

/**
 * Minimal engine interface — just what we need to save bytes. Defined
 * structurally so we don't take a hard dep on `@vcad/kernel-wasm` from here.
 */
export interface CrdtSaveableEngine {
  save(): Uint8Array;
}

/**
 * Build a canonical `VcadFile` from live store state. The sole entry point
 * for writing a CRDT-kind file; callers should prefer this over constructing
 * the union variant by hand so invariants (engine present, loon precedence)
 * stay consistent across auto-save, manual save, sync, and backfill.
 *
 * Returns `null` if neither loon source nor a CRDT engine is available —
 * callers should retry once the engine finishes booting.
 */
export function buildVcadFileFromState(state: {
  loonSource?: string | null;
  _crdtEngine?: CrdtSaveableEngine | null;
  document?: Document;
  parts?: PartInfo[];
  nextNodeId?: number;
}): VcadFile | null {
  // Loon-authored docs keep loon as source of truth. The derived Document /
  // parts snapshot rides along for consumers that want the materialized view
  // without re-evaluating (e.g. thumbnails at bootstrap).
  if (state.loonSource) {
    return {
      kind: "loon",
      version: "0.3",
      loonSource: state.loonSource,
      document: state.document ?? createDocument(),
      parts: state.parts ?? [],
      nextNodeId: state.nextNodeId ?? 1,
    };
  }
  if (state._crdtEngine) {
    return {
      kind: "crdt",
      version: "0.4",
      crdtBytes: state._crdtEngine.save(),
    };
  }
  return null;
}

/**
 * Serialize the live document state to text for on-disk / in-memory storage.
 *
 * Preference order:
 *  1. `loonSource` — preserved verbatim so round-tripping loon docs stays
 *     round-trippable at the source level.
 *  2. `crdtBytes` (or `_crdtEngine.save()`) — CRDT JSON decoded as UTF-8.
 *     This is the canonical path.
 *
 * VCode is NOT a serialization target — see `toVCode` for one-way export.
 * Falling back to VCode on save was the class of bug we're removing.
 */
export function serializeDocument(state: {
  crdtBytes?: Uint8Array | null;
  _crdtEngine?: CrdtSaveableEngine | null;
  loonSource?: string | null;
}): string {
  if (state.loonSource) return state.loonSource;
  const bytes =
    state.crdtBytes && state.crdtBytes.byteLength > 0
      ? state.crdtBytes
      : state._crdtEngine?.save();
  if (bytes && bytes.byteLength > 0) {
    return new TextDecoder().decode(bytes);
  }
  throw new Error(
    "serializeDocument: no CRDT bytes or loon source to serialize — engine not initialized?",
  );
}

/**
 * Parse a `.vcad` file.
 *
 * Format detection (in order):
 *  - starts with `{"replica_id"...` → v0.4 CRDT bytes
 *  - starts with `[` or `;` → v0.3 loon
 *  - starts with `{` → v0.1 legacy IR JSON
 *  - anything else → v0.2 VCode
 *
 * @param evalLoon - Required for loon format when WASM is unavailable.
 */
export function parseVcadFile(
  content: string,
  evalLoon?: (source: string) => string,
): VcadFile {
  const trimmed = content.trim();

  // v0.4 CRDT — detect before other JSON shapes. This is a fast-path that
  // avoids calling WASM's legacy parser (which would reject CRDT as invalid
  // Document JSON).
  if (isCrdtJson(trimmed)) {
    return parseCrdtVcadFile(trimmed);
  }

  // Try WASM path for legacy formats — includes bundled loon evaluator.
  if (wasmModule?.parseVcadFile) {
    try {
      const result = wasmModule.parseVcadFile(trimmed) as unknown;
      return wrapLegacyWasmResult(result);
    } catch (e) {
      console.warn("[CORE] WASM parseVcadFile failed, using TS fallback:", e);
    }
  }

  return parseVcadFileTS(trimmed, evalLoon);
}

/**
 * Quick-check whether a trimmed string looks like CRDT JSON. Cheaper than
 * a full JSON parse; we accept a small false-positive risk for payloads that
 * happen to contain `"replica_id"` and `"ops"` — those would fail the
 * subsequent CRDT load and surface as an actionable error.
 */
function isCrdtJson(trimmed: string): boolean {
  if (!trimmed.startsWith("{")) return false;
  return trimmed.includes('"replica_id"') && trimmed.includes('"ops"');
}

/**
 * Wrap a Rust-side WASM `parseVcadFile` result in the new tagged envelope.
 *
 * The Rust side hasn't been migrated to the tagged union yet; it still
 * returns the flat legacy `{version, document, parts, ...}` shape.
 */
function wrapLegacyWasmResult(result: unknown): VcadFile {
  if (!result || typeof result !== "object") {
    throw new Error("Invalid .vcad file: WASM parser returned non-object");
  }
  const obj = result as Record<string, unknown>;
  const loonSource = typeof obj.loonSource === "string" ? obj.loonSource : null;
  const document = obj.document as Document;
  const parts = (obj.parts ?? []) as PartInfo[];
  const nextNodeId = typeof obj.nextNodeId === "number" ? obj.nextNodeId : 0;
  const nextPartNum =
    typeof obj.nextPartNum === "number" ? obj.nextPartNum : undefined;
  const consumedParts =
    (obj.consumedParts as Record<string, PartInfo> | undefined) ?? {};
  const version = typeof obj.version === "string" ? obj.version : "0.1";

  if (loonSource) {
    return {
      kind: "loon",
      version: "0.3",
      loonSource,
      document,
      parts,
      nextNodeId,
      nextPartNum,
    };
  }
  return {
    kind: "legacy",
    version: (version === "0.2" ? "0.2" : "0.1") as "0.1" | "0.2",
    document,
    parts,
    consumedParts,
    nextNodeId,
    nextPartNum,
  };
}

/** TypeScript fallback for parseVcadFile (WASM unavailable). */
function parseVcadFileTS(
  content: string,
  evalLoon?: (source: string) => string,
): VcadFile {
  const trimmed = content.trim();

  // CRDT already handled upstream; this is the non-CRDT fallback.
  if (trimmed.startsWith("{")) {
    return parseJsonVcadFile(trimmed);
  }

  if (trimmed.startsWith("[") || trimmed.startsWith(";")) {
    return parseLoonVcadFile(trimmed, evalLoon);
  }

  return parseVCodeFile(trimmed);
}

// Hard caps on shape/size — enough for any realistic document, small enough
// to keep a malicious .vcad from allocating gigabytes or blowing the stack.
const MAX_VCAD_BYTES = 64 * 1024 * 1024;
const MAX_VCAD_NODES = 500_000;
const MAX_VCAD_PARTS = 50_000;

// Use Object.prototype.hasOwnProperty.call so `obj` retains its type rather
// than being narrowed to `never` after "key" in obj guards (TS collapses
// Record<string, unknown> minus a specific key to never).
function hasOwn(obj: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(obj, key);
}

/** Parse v0.4 CRDT bytes. Input is CRDT JSON text; we carry the bytes forward. */
function parseCrdtVcadFile(json: string): VcadFileCrdt {
  if (json.length > MAX_VCAD_BYTES) {
    throw new Error("Invalid .vcad file: exceeds size limit");
  }
  // Pre-flight parse so we surface malformed JSON at parse time rather than
  // downstream at engine.load(). Cheap compared to the actual CRDT load.
  try {
    JSON.parse(json);
  } catch (e) {
    throw new Error(`Invalid v0.4 .vcad file: ${(e as Error).message}`);
  }
  return {
    kind: "crdt",
    version: "0.4",
    crdtBytes: new TextEncoder().encode(json),
  };
}

/** Parse legacy JSON format (v0.1). */
function parseJsonVcadFile(json: string): VcadFileLegacy {
  if (json.length > MAX_VCAD_BYTES) {
    throw new Error("Invalid .vcad file: exceeds size limit");
  }
  const raw = JSON.parse(json) as unknown;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Invalid .vcad file: expected an object");
  }
  const obj = raw as Record<string, unknown>;

  // Prototype-pollution defense: refuse any top-level key that could
  // reassign Object.prototype downstream.
  if (hasOwn(obj, "__proto__") || hasOwn(obj, "constructor") || hasOwn(obj, "prototype")) {
    throw new Error("Invalid .vcad file: forbidden key in root");
  }

  const document = obj.document;
  if (!document || typeof document !== "object" || Array.isArray(document)) {
    throw new Error("Invalid .vcad file: missing or malformed document");
  }
  const doc = document as Record<string, unknown>;
  const docNodes = doc.nodes;
  if (!docNodes || typeof docNodes !== "object" || Array.isArray(docNodes)) {
    throw new Error("Invalid .vcad file: document.nodes must be an object");
  }
  const nodes = docNodes as Record<string, unknown>;
  if (hasOwn(nodes, "__proto__") || hasOwn(nodes, "constructor")) {
    throw new Error("Invalid .vcad file: forbidden key in document.nodes");
  }
  if (Object.keys(nodes).length > MAX_VCAD_NODES) {
    throw new Error("Invalid .vcad file: too many nodes");
  }
  if (!Array.isArray(doc.roots)) {
    throw new Error("Invalid .vcad file: document.roots must be an array");
  }

  const parts = obj.parts;
  if (!Array.isArray(parts)) {
    throw new Error("Invalid .vcad file: parts must be an array");
  }
  if (parts.length > MAX_VCAD_PARTS) {
    throw new Error("Invalid .vcad file: too many parts");
  }

  const nextNodeId = obj.nextNodeId;
  if (typeof nextNodeId !== "number" || !Number.isFinite(nextNodeId)) {
    throw new Error("Invalid .vcad file: missing or invalid nextNodeId");
  }

  return {
    kind: "legacy",
    version: "0.1",
    document: document as Document,
    parts: parts as PartInfo[],
    consumedParts: (obj.consumedParts as Record<string, PartInfo> | undefined) ?? {},
    nextNodeId,
    nextPartNum: typeof obj.nextPartNum === "number" ? obj.nextPartNum : undefined,
  };
}

/** Parse loon format (v0.3). */
function parseLoonVcadFile(
  source: string,
  evalLoon?: (source: string) => string,
): VcadFileLoon {
  if (!evalLoon) {
    throw new Error("Loon format detected but no evaluator provided. Engine may not be ready.");
  }
  const json = evalLoon(source);
  const document: Document = JSON.parse(json);
  const parts = deriveParts(document);
  const { nextNodeId, nextPartNum } = computeNextIds(document, parts);

  return {
    kind: "loon",
    version: "0.3",
    loonSource: source,
    document,
    parts,
    nextNodeId,
    nextPartNum,
  };
}

/** Parse VCode format (v0.2). */
function parseVCodeFile(compact: string): VcadFileLegacy {
  const document = fromVCode(compact);
  const parts = deriveParts(document);
  const { nextNodeId, nextPartNum } = computeNextIds(document, parts);

  return {
    kind: "legacy",
    version: "0.2",
    document,
    parts,
    consumedParts: {},
    nextNodeId,
    nextPartNum,
  };
}

/**
 * Derive PartInfo[] from a Document by analyzing the node graph.
 *
 * Uses Rust WASM when available, falls back to TypeScript.
 */
export function deriveParts(document: Document): PartInfo[] {
  if (wasmModule?.deriveParts) {
    try {
      return wasmModule.deriveParts(JSON.stringify(document)) as PartInfo[];
    } catch (e) {
      console.warn("[CORE] WASM deriveParts failed, using TS fallback:", e);
    }
  }
  return derivePartsTS(document);
}

function derivePartsTS(document: Document): PartInfo[] {
  const parts: PartInfo[] = [];
  let partNum = 1;

  // Build a set of nodes that are referenced as children (not roots in terms of parts)
  const childNodes = new Set<NodeId>();
  for (const key of Object.keys(document.nodes)) {
    const node = document.nodes[key];
    if (!node) continue;
    const children = getChildNodes(node.op);
    for (const child of children) {
      childNodes.add(child);
    }
  }

  // Process each scene root
  for (const root of document.roots) {
    const rootNode = document.nodes[String(root.root)];
    if (!rootNode) continue;

    const partInfo = derivePartFromRoot(document, root.root, partNum);
    if (partInfo) {
      parts.push(partInfo);
      partNum++;
    }
  }

  return parts;
}

/**
 * Derive a single PartInfo from a scene root node
 */
function derivePartFromRoot(
  document: Document,
  rootNodeId: NodeId,
  partNum: number
): PartInfo | null {
  const chain = walkTransformChain(document, rootNodeId);
  if (!chain) return null;

  const { translateNodeId, rotateNodeId, scaleNodeId, coreNodeId, coreOp } = chain;

  // Get name from the translate node (where names are typically stored)
  const translateNode = document.nodes[String(translateNodeId)];
  const name = translateNode?.name ?? `Part ${partNum}`;
  const partId = `part-${partNum}`;

  // Determine part kind based on core operation
  const kind = coreOp.type;

  switch (kind) {
    case "Cube":
    case "Cylinder":
    case "Sphere":
      return {
        id: partId,
        name,
        kind: kind.toLowerCase() as PrimitiveKind,
        primitiveNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Union":
    case "Difference":
    case "Intersection": {
      const booleanType = kind.toLowerCase() as "union" | "difference" | "intersection";
      return {
        id: partId,
        name,
        kind: "boolean",
        booleanType,
        booleanNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
        sourcePartIds: ["unknown", "unknown"], // Can't derive original part IDs
      };
    }

    case "Extrude":
      return {
        id: partId,
        name,
        kind: "extrude",
        sketchNodeId: coreOp.sketch,
        extrudeNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Revolve":
      return {
        id: partId,
        name,
        kind: "revolve",
        sketchNodeId: coreOp.sketch,
        revolveNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Sweep":
      return {
        id: partId,
        name,
        kind: "sweep",
        sketchNodeId: coreOp.sketch,
        sweepNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Loft":
      return {
        id: partId,
        name,
        kind: "loft",
        sketchNodeIds: coreOp.sketches,
        loftNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "ImportedMesh":
      return {
        id: partId,
        name,
        kind: "imported-mesh",
        meshNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Fillet":
      return {
        id: partId,
        name,
        kind: "fillet",
        sourcePartId: "unknown",
        filletNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Chamfer":
      return {
        id: partId,
        name,
        kind: "chamfer",
        sourcePartId: "unknown",
        chamferNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "Shell":
      return {
        id: partId,
        name,
        kind: "shell",
        sourcePartId: "unknown",
        shellNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "LinearPattern":
      return {
        id: partId,
        name,
        kind: "linear-pattern",
        sourcePartId: "unknown",
        patternNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "CircularPattern":
      return {
        id: partId,
        name,
        kind: "circular-pattern",
        sourcePartId: "unknown",
        patternNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "PcbBoard":
      return {
        id: partId,
        name,
        kind: "pcb-board",
        boardNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    case "EmbroideryPattern":
      return {
        id: partId,
        name,
        kind: "embroidery-pattern",
        patternNodeId: coreNodeId,
        scaleNodeId,
        rotateNodeId,
        translateNodeId,
      };

    // Note: Mirror is in PartInfo types but not yet in IR CsgOp

    default:
      // Unknown op type, skip
      return null;
  }
}

interface TransformChain {
  translateNodeId: NodeId;
  rotateNodeId: NodeId;
  scaleNodeId: NodeId;
  coreNodeId: NodeId;
  coreOp: CsgOp;
}

/**
 * Walk backward from a root node through the transform chain.
 * Expected pattern: root(Translate) -> Rotate -> Scale -> core operation
 *
 * If transforms are missing, we create virtual identity transforms.
 */
function walkTransformChain(document: Document, rootNodeId: NodeId): TransformChain | null {
  const rootNode = document.nodes[String(rootNodeId)];
  if (!rootNode) return null;

  // translateNodeId should always match the document root entry.
  let translateNodeId = rootNodeId;
  let rotateNodeId = rootNodeId;
  let scaleNodeId = rootNodeId;

  let currentNode = rootNode;
  let coreNodeId = rootNodeId;
  let coreOp = rootNode.op;

  let sawRotate = false;
  let sawScale = false;

  // Walk down through any contiguous transform nodes in the chain.
  while (
    currentNode.op.type === "Translate" ||
    currentNode.op.type === "Rotate" ||
    currentNode.op.type === "Scale"
  ) {
    if (currentNode.op.type === "Rotate" && !sawRotate) {
      rotateNodeId = currentNode.id;
      sawRotate = true;
    }
    if (currentNode.op.type === "Scale" && !sawScale) {
      scaleNodeId = currentNode.id;
      sawScale = true;
    }

    const childNode = document.nodes[String(currentNode.op.child)];
    if (!childNode) {
      // Dangling transform; treat this transform as the core.
      coreNodeId = currentNode.id;
      coreOp = currentNode.op;
      return { translateNodeId, rotateNodeId, scaleNodeId, coreNodeId, coreOp };
    }

    currentNode = childNode;
    coreNodeId = currentNode.id;
    coreOp = currentNode.op;
  }

  return { translateNodeId, rotateNodeId, scaleNodeId, coreNodeId, coreOp };
}

/**
 * Get child node IDs from an operation
 */
function getChildNodes(op: CsgOp): NodeId[] {
  switch (op.type) {
    case "Translate":
    case "Rotate":
    case "Scale":
    case "LinearPattern":
    case "CircularPattern":
    case "Fillet":
    case "Chamfer":
    case "EdgeBlendLoft":
    case "Shell":
      return [op.child];
    case "Union":
    case "Difference":
    case "Intersection":
      return [op.left, op.right];
    case "Extrude":
    case "Revolve":
    case "Sweep":
      return [op.sketch];
    case "Loft":
      return op.sketches;
    default:
      return [];
  }
}

/**
 * Compute the next available node ID and part number for a freshly loaded
 * Document — used when constructing a {@link VcadFile} from non-`.vcad`
 * sources (e.g. the URDF importer in the web app).
 */
export function computeNextIds(
  document: Document,
  parts: PartInfo[]
): { nextNodeId: number; nextPartNum: number } {
  // Find max node ID
  let maxNodeId = 0;
  for (const key of Object.keys(document.nodes)) {
    const id = parseInt(key, 10);
    if (!isNaN(id) && id > maxNodeId) {
      maxNodeId = id;
    }
  }

  // Find max part number from part IDs
  let maxPartNum = 0;
  for (const part of parts) {
    const match = part.id.match(/^part-(\d+)$/);
    if (match && match[1]) {
      const num = parseInt(match[1], 10);
      if (num > maxPartNum) maxPartNum = num;
    }
  }

  return {
    nextNodeId: maxNodeId + 1,
    nextPartNum: maxPartNum + 1,
  };
}
