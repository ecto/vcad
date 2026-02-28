import type { Document, NodeId, CsgOp, Node } from "@vcad/ir";
import { toVCode, fromVCode, createDocument } from "@vcad/ir";
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

export interface VcadFile {
  version: string;
  document: Document;
  parts: PartInfo[];
  consumedParts?: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum?: number;
  loonSource?: string | null;
}

/**
 * Serialize document — uses loon source if available, otherwise VCode.
 */
export function serializeDocument(state: {
  document: Document;
  parts: PartInfo[];
  consumedParts: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum?: number;
  loonSource?: string | null;
}): string {
  if (state.loonSource) return state.loonSource;
  return toVCode(state.document);
}

/**
 * Parse a .vcad file (supports JSON v0.1, compact v0.2, and loon v0.3 formats)
 *
 * Uses Rust WASM when available (includes built-in loon evaluator).
 * Falls back to TypeScript implementation.
 *
 * @param evalLoon - Optional callback to evaluate loon source → JSON Document string.
 *   Required for TS fallback when content is loon format.
 */
export function parseVcadFile(
  content: string,
  evalLoon?: (source: string) => string,
): VcadFile {
  // Try WASM path — bundles loon evaluator so no callback needed
  if (wasmModule?.parseVcadFile) {
    try {
      const result = wasmModule.parseVcadFile(content) as VcadFile;
      return result;
    } catch (e) {
      console.warn("[CORE] WASM parseVcadFile failed, using TS fallback:", e);
    }
  }

  return parseVcadFileTS(content, evalLoon);
}

/** TypeScript fallback for parseVcadFile. */
function parseVcadFileTS(
  content: string,
  evalLoon?: (source: string) => string,
): VcadFile {
  const trimmed = content.trim();

  // Detect format: JSON starts with '{', loon starts with '[' or ';'
  if (trimmed.startsWith("{")) {
    return parseJsonVcadFile(trimmed);
  }

  if (trimmed.startsWith("[") || trimmed.startsWith(";")) {
    return parseLoonVcadFile(trimmed, evalLoon);
  }

  return parseVCodeFile(trimmed);
}

/**
 * Parse legacy JSON format (v0.1)
 */
function parseJsonVcadFile(json: string): VcadFile {
  const data = JSON.parse(json) as VcadFile;

  // Basic validation
  if (!data.document || !Array.isArray(data.parts)) {
    throw new Error("Invalid .vcad file: missing document or parts");
  }
  if (typeof data.nextNodeId !== "number") {
    throw new Error("Invalid .vcad file: missing nextNodeId");
  }

  return data;
}

/**
 * Parse loon format (v0.3)
 */
function parseLoonVcadFile(
  source: string,
  evalLoon?: (source: string) => string,
): VcadFile {
  if (!evalLoon) {
    throw new Error("Loon format detected but no evaluator provided. Engine may not be ready.");
  }
  const json = evalLoon(source);
  const document: Document = JSON.parse(json);
  const parts = deriveParts(document);
  const { nextNodeId, nextPartNum } = computeNextIds(document, parts);

  return {
    version: "0.3",
    document,
    parts,
    consumedParts: {},
    nextNodeId,
    nextPartNum,
    loonSource: source,
  };
}

/**
 * Parse VCode format (v0.2)
 */
function parseVCodeFile(compact: string): VcadFile {
  const document = fromVCode(compact);
  const parts = deriveParts(document);
  const { nextNodeId, nextPartNum } = computeNextIds(document, parts);

  return {
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
 * Compute the next available node ID and part number
 */
function computeNextIds(
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
