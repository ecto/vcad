import type {
  Document,
  Node,
  NodeId,
  CsgOp,
  Sketch2DOp,
  Text2DOp,
  SketchSegment2D,
  SweepOp,
  LoftOp,
  Transform3D,
  ImportedMeshOp,
  PcbBoardOp,
} from "@vcad/ir";
import type {
  EvaluatedScene,
  EvaluatedPartDef,
  EvaluatedInstance,
  TriangleMesh,
} from "./mesh.js";
import type { Solid } from "@vcad/kernel-wasm";
import { solveForwardKinematics } from "./kinematics.js";

/** Debug flag - set to true to enable verbose console logging */
const DEBUG_EVAL = false;

/** Options for document evaluation */
export interface EvaluateOptions {
  /** Skip O(n²) clash detection for faster updates during parametric editing */
  skipClashDetection?: boolean;
}

/** Type for the kernel module */
interface KernelModule {
  Solid: typeof Solid;
  evaluateDocument?: (docJson: string, skipClashDetection: boolean) => unknown;
}

/** Shape of the WASM evaluator result */
interface WasmEvaluatedScene {
  parts: Array<{ mesh: WasmMesh; material: string }>;
  partDefs?: Array<{ id: string; mesh: WasmMesh }>;
  instances?: Array<{
    instance_id: string;
    part_def_id: string;
    name?: string;
    mesh: WasmMesh;
    material: string;
    transform?: Transform3D;
  }>;
  clashes: Array<WasmMesh>;
}

interface WasmMesh {
  positions: number[];
  indices: number[];
  normals?: number[];
}

/** Convert a WASM mesh to a TriangleMesh (typed arrays). */
function wasmMeshToTriangleMesh(m: WasmMesh): TriangleMesh {
  return {
    positions: new Float32Array(m.positions),
    indices: new Uint32Array(m.indices),
    normals: m.normals ? new Float32Array(m.normals) : undefined,
  };
}

/**
 * Evaluate a vcad IR Document into an EvaluatedScene.
 *
 * Prefers the Rust WASM evaluator (evaluateDocument) when available, which
 * handles ALL CsgOp variants including Sketch2D, Extrude, Revolve, Sweep,
 * Loft, Text2D, ImportedMesh, assembly with forward kinematics, and clash
 * detection.
 *
 * Falls back to the TypeScript evaluator when the WASM evaluator is not
 * available (e.g., older WASM builds).
 */
export function evaluateDocument(
  doc: Document,
  kernel: KernelModule,
  options: EvaluateOptions = {},
): EvaluatedScene {
  // Try the Rust WASM evaluator first
  if (kernel.evaluateDocument) {
    try {
      const docJson = JSON.stringify(doc);
      const result = kernel.evaluateDocument(
        docJson,
        options.skipClashDetection ?? false,
      ) as WasmEvaluatedScene;

      return {
        parts: result.parts.map((p) => ({
          mesh: wasmMeshToTriangleMesh(p.mesh),
          material: p.material,
        })),
        partDefs: result.partDefs?.map((pd) => ({
          id: pd.id,
          mesh: wasmMeshToTriangleMesh(pd.mesh),
        })),
        instances: result.instances?.map((inst) => ({
          instanceId: inst.instance_id,
          partDefId: inst.part_def_id,
          name: inst.name,
          mesh: wasmMeshToTriangleMesh(inst.mesh),
          material: inst.material,
          transform: inst.transform,
        })),
        clashes: result.clashes.map(wasmMeshToTriangleMesh),
      };
    } catch (e) {
      console.warn("[ENGINE] WASM evaluateDocument failed, falling back to TS:", e);
      // Fall through to TS evaluator
    }
  }

  // Fallback: TypeScript evaluator
  return evaluateDocumentTS(doc, kernel, options);
}

// =========================================================================
// TypeScript fallback evaluator (original implementation)
// =========================================================================

/** Convert IR sketch segment to WASM format */
function convertSegment(seg: SketchSegment2D) {
  if (seg.type === "Line") {
    return {
      type: "Line" as const,
      start: [seg.start.x, seg.start.y],
      end: [seg.end.x, seg.end.y],
    };
  } else {
    return {
      type: "Arc" as const,
      start: [seg.start.x, seg.start.y],
      end: [seg.end.x, seg.end.y],
      center: [seg.center.x, seg.center.y],
      ccw: seg.ccw,
    };
  }
}

/** Convert IR Sketch2D op to WASM profile format */
function convertSketchToProfile(op: Sketch2DOp) {
  return {
    origin: [op.origin.x, op.origin.y, op.origin.z],
    x_dir: [op.x_dir.x, op.x_dir.y, op.x_dir.z],
    y_dir: [op.y_dir.x, op.y_dir.y, op.y_dir.z],
    segments: op.segments.map(convertSegment),
  };
}

/** Extract a TriangleMesh from a Solid. */
function solidToMesh(solid: Solid): TriangleMesh {
  const meshData = solid.getMesh();
  const positions = new Float32Array(meshData.positions);
  const indices = new Uint32Array(meshData.indices);

  // Validate indices - check for out-of-bounds references
  const numVertices = positions.length / 3;
  let hasInvalidIndices = false;
  for (let i = 0; i < indices.length; i++) {
    if (indices[i] >= numVertices) {
      hasInvalidIndices = true;
      break;
    }
  }

  if (hasInvalidIndices) {
    const validIndices: number[] = [];
    for (let i = 0; i < indices.length; i += 3) {
      const i0 = indices[i];
      const i1 = indices[i + 1];
      const i2 = indices[i + 2];
      if (i0 < numVertices && i1 < numVertices && i2 < numVertices) {
        validIndices.push(i0, i1, i2);
      }
    }
    return {
      positions,
      indices: new Uint32Array(validIndices),
      normals: meshData.normals ? new Float32Array(meshData.normals) : undefined,
    };
  }

  return {
    positions,
    indices,
    normals: meshData.normals ? new Float32Array(meshData.normals) : undefined,
  };
}

/** Transform info extracted from node chain */
interface TransformInfo {
  translate: { x: number; y: number; z: number };
  rotate: { x: number; y: number; z: number };
  scale: { x: number; y: number; z: number };
}

/**
 * Find an ImportedMesh in the node chain and extract transforms.
 */
function findImportedMesh(
  rootId: NodeId,
  nodes: Record<string, Node>,
): { mesh: ImportedMeshOp; transform: TransformInfo } | null {
  const transform: TransformInfo = {
    translate: { x: 0, y: 0, z: 0 },
    rotate: { x: 0, y: 0, z: 0 },
    scale: { x: 1, y: 1, z: 1 },
  };

  let current = rootId;
  while (true) {
    const node = nodes[String(current)];
    if (!node) return null;

    if (node.op.type === "ImportedMesh") {
      return { mesh: node.op, transform };
    }

    if (node.op.type === "Translate") {
      transform.translate = node.op.offset;
      current = node.op.child;
    } else if (node.op.type === "Rotate") {
      transform.rotate = node.op.angles;
      current = node.op.child;
    } else if (node.op.type === "Scale") {
      transform.scale = node.op.factor;
      current = node.op.child;
    } else {
      return null;
    }
  }
}

/**
 * Find a PcbBoard in the node chain and extract transforms.
 */
function findPcbBoard(
  rootId: NodeId,
  nodes: Record<string, Node>,
): { board: PcbBoardOp; transform: TransformInfo } | null {
  const transform: TransformInfo = {
    translate: { x: 0, y: 0, z: 0 },
    rotate: { x: 0, y: 0, z: 0 },
    scale: { x: 1, y: 1, z: 1 },
  };

  let current = rootId;
  while (true) {
    const node = nodes[String(current)];
    if (!node) return null;

    if (node.op.type === "PcbBoard") {
      return { board: node.op, transform };
    }

    if (node.op.type === "Translate") {
      transform.translate = node.op.offset;
      current = node.op.child;
    } else if (node.op.type === "Rotate") {
      transform.rotate = node.op.angles;
      current = node.op.child;
    } else if (node.op.type === "Scale") {
      transform.scale = node.op.factor;
      current = node.op.child;
    } else {
      return null;
    }
  }
}

/**
 * Generate a simple extruded mesh from a PCB board outline (ear-clip triangulation + side faces).
 */
function pcbBoardToMesh(op: PcbBoardOp): TriangleMesh {
  const verts = op.board.outline.vertices;
  const thickness = op.board.outline.thickness;
  if (verts.length < 3) {
    return { positions: new Float32Array(0), indices: new Uint32Array(0) };
  }

  const n = verts.length;
  // Simple ear-clip triangulation for convex/simple polygon
  const topPositions: number[] = [];
  const botPositions: number[] = [];
  for (const v of verts) {
    topPositions.push(v.x, v.y, thickness);
    botPositions.push(v.x, v.y, 0);
  }

  const allPositions: number[] = [];
  const allIndices: number[] = [];

  // Top face (fan triangulation)
  const topOffset = 0;
  for (const v of verts) {
    allPositions.push(v.x, v.y, thickness);
  }
  for (let i = 1; i < n - 1; i++) {
    allIndices.push(topOffset, topOffset + i, topOffset + i + 1);
  }

  // Bottom face (reversed winding)
  const botOffset = n;
  for (const v of verts) {
    allPositions.push(v.x, v.y, 0);
  }
  for (let i = 1; i < n - 1; i++) {
    allIndices.push(botOffset, botOffset + i + 1, botOffset + i);
  }

  // Side faces
  const sideOffset = n * 2;
  for (let i = 0; i < n; i++) {
    const next = (i + 1) % n;
    const base = sideOffset + i * 4;
    // Four vertices per side quad
    allPositions.push(verts[i].x, verts[i].y, 0);
    allPositions.push(verts[next].x, verts[next].y, 0);
    allPositions.push(verts[next].x, verts[next].y, thickness);
    allPositions.push(verts[i].x, verts[i].y, thickness);
    allIndices.push(base, base + 1, base + 2);
    allIndices.push(base, base + 2, base + 3);
  }

  return {
    positions: new Float32Array(allPositions),
    indices: new Uint32Array(allIndices),
  };
}

/**
 * Apply a transform to mesh positions.
 */
function transformMesh(
  mesh: TriangleMesh,
  transform: TransformInfo,
): TriangleMesh {
  const { translate, rotate, scale } = transform;
  const positions = new Float32Array(mesh.positions.length);

  const rx = (rotate.x * Math.PI) / 180;
  const ry = (rotate.y * Math.PI) / 180;
  const rz = (rotate.z * Math.PI) / 180;

  const cx = Math.cos(rx), sx = Math.sin(rx);
  const cy = Math.cos(ry), sy = Math.sin(ry);
  const cz = Math.cos(rz), sz = Math.sin(rz);

  const m00 = cy * cz;
  const m01 = sx * sy * cz - cx * sz;
  const m02 = cx * sy * cz + sx * sz;
  const m10 = cy * sz;
  const m11 = sx * sy * sz + cx * cz;
  const m12 = cx * sy * sz - sx * cz;
  const m20 = -sy;
  const m21 = sx * cy;
  const m22 = cx * cy;

  for (let i = 0; i < mesh.positions.length; i += 3) {
    let x = mesh.positions[i] * scale.x;
    let y = mesh.positions[i + 1] * scale.y;
    let z = mesh.positions[i + 2] * scale.z;

    const rx2 = m00 * x + m01 * y + m02 * z;
    const ry2 = m10 * x + m11 * y + m12 * z;
    const rz2 = m20 * x + m21 * y + m22 * z;

    positions[i] = rx2 + translate.x;
    positions[i + 1] = ry2 + translate.y;
    positions[i + 2] = rz2 + translate.z;
  }

  let normals = mesh.normals;
  if (mesh.normals) {
    normals = new Float32Array(mesh.normals.length);
    for (let i = 0; i < mesh.normals.length; i += 3) {
      const nx = mesh.normals[i];
      const ny = mesh.normals[i + 1];
      const nz = mesh.normals[i + 2];

      normals[i] = m00 * nx + m01 * ny + m02 * nz;
      normals[i + 1] = m10 * nx + m11 * ny + m12 * nz;
      normals[i + 2] = m20 * nx + m21 * ny + m22 * nz;
    }
  }

  return { positions, indices: mesh.indices, normals };
}

/**
 * TypeScript fallback evaluator (original implementation).
 */
function evaluateDocumentTS(
  doc: Document,
  kernel: KernelModule,
  options: EvaluateOptions = {},
): EvaluatedScene {
  const { Solid } = kernel;
  const cache = new Map<NodeId, Solid>();

  // Traditional mode: evaluate roots (filter out hidden parts)
  const visibleRoots = doc.roots.filter((entry) => entry.visible !== false);
  const solids: Solid[] = [];
  const parts = visibleRoots.map((entry) => {
    // Check if this is a PcbBoard (doesn't go through Solid pipeline)
    const pcbBoard = findPcbBoard(entry.root, doc.nodes);
    if (pcbBoard) {
      const baseMesh = pcbBoardToMesh(pcbBoard.board);
      const mesh = transformMesh(baseMesh, pcbBoard.transform);
      solids.push(Solid.empty());
      return { mesh, material: entry.material };
    }

    // Check if this is an imported mesh (doesn't go through Solid pipeline)
    const imported = findImportedMesh(entry.root, doc.nodes);
    if (imported) {
      const baseMesh: TriangleMesh = {
        positions: new Float32Array(imported.mesh.positions),
        indices: new Uint32Array(imported.mesh.indices),
        normals: imported.mesh.normals ? new Float32Array(imported.mesh.normals) : undefined,
      };
      const mesh = transformMesh(baseMesh, imported.transform);
      solids.push(Solid.empty());
      return { mesh, material: entry.material };
    }

    const solid = evaluateNode(entry.root, doc.nodes, Solid, cache, 0);
    const mesh = solidToMesh(solid);
    solids.push(solid);
    return {
      mesh,
      material: entry.material,
      solid: solid,
    };
  });

  // Assembly mode
  let evaluatedPartDefs: EvaluatedPartDef[] | undefined;
  let evaluatedInstances: EvaluatedInstance[] | undefined;

  if (doc.partDefs && Object.keys(doc.partDefs).length > 0 && doc.instances && doc.instances.length > 0) {
    const worldTransforms = solveForwardKinematics(doc);

    const partDefMeshes = new Map<string, TriangleMesh>();
    evaluatedPartDefs = [];
    for (const [id, partDef] of Object.entries(doc.partDefs)) {
      const solid = evaluateNode(partDef.root, doc.nodes, Solid, cache, 0);
      const mesh = solidToMesh(solid);
      partDefMeshes.set(id, mesh);
      evaluatedPartDefs.push({ id, mesh });
    }

    evaluatedInstances = [];
    for (const instance of doc.instances) {
      const mesh = partDefMeshes.get(instance.partDefId);
      if (!mesh) continue;

      const worldTransform = worldTransforms.get(instance.id) ?? instance.transform;
      const partDef = doc.partDefs[instance.partDefId];
      const material = instance.material ?? partDef?.defaultMaterial ?? "default";

      evaluatedInstances.push({
        instanceId: instance.id,
        partDefId: instance.partDefId,
        name: instance.name,
        mesh,
        material,
        transform: worldTransform,
      });
    }
  }

  // Clash detection
  const clashes: TriangleMesh[] = [];
  if (!options.skipClashDetection) {
    for (let i = 0; i < solids.length; i++) {
      for (let j = i + 1; j < solids.length; j++) {
        const intersection = solids[i].intersection(solids[j]);
        if (!intersection.isEmpty()) {
          const meshData = intersection.getMesh();
          if (meshData.positions.length > 0) {
            clashes.push({
              positions: new Float32Array(meshData.positions),
              indices: new Uint32Array(meshData.indices),
              normals: meshData.normals
                ? new Float32Array(meshData.normals)
                : undefined,
            });
          }
        }
      }
    }
  }

  return {
    parts,
    partDefs: evaluatedPartDefs,
    instances: evaluatedInstances,
    clashes,
  };
}

function evaluateNode(
  nodeId: NodeId,
  nodes: Record<string, Node>,
  Solid: typeof import("@vcad/kernel-wasm").Solid,
  cache: Map<NodeId, import("@vcad/kernel-wasm").Solid>,
  depth = 0,
): import("@vcad/kernel-wasm").Solid {
  const cached = cache.get(nodeId);
  if (cached) return cached;

  const node = nodes[String(nodeId)];
  if (!node) throw new Error(`Missing node: ${nodeId}`);

  const result = evaluateOp(node.op, nodes, Solid, cache, depth);
  cache.set(nodeId, result);
  return result;
}

function evaluateOp(
  op: CsgOp,
  nodes: Record<string, Node>,
  Solid: typeof import("@vcad/kernel-wasm").Solid,
  cache: Map<NodeId, import("@vcad/kernel-wasm").Solid>,
  depth = 0,
): import("@vcad/kernel-wasm").Solid {
  switch (op.type) {
    case "Cube":
      return Solid.cube(op.size.x, op.size.y, op.size.z);

    case "Cylinder":
      return Solid.cylinder(op.radius, op.height, op.segments || undefined);

    case "Sphere":
      return Solid.sphere(op.radius, op.segments || undefined);

    case "Cone":
      return Solid.cone(
        op.radius_bottom,
        op.radius_top,
        op.height,
        op.segments || undefined,
      );

    case "Empty":
      return Solid.empty();

    case "Union": {
      const left = evaluateNode(op.left, nodes, Solid, cache, depth + 1);
      const right = evaluateNode(op.right, nodes, Solid, cache, depth + 1);
      return left.union(right);
    }

    case "Difference": {
      const left = evaluateNode(op.left, nodes, Solid, cache, depth + 1);
      const right = evaluateNode(op.right, nodes, Solid, cache, depth + 1);
      return left.difference(right);
    }

    case "Intersection": {
      const left = evaluateNode(op.left, nodes, Solid, cache, depth + 1);
      const right = evaluateNode(op.right, nodes, Solid, cache, depth + 1);
      return left.intersection(right);
    }

    case "Translate": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.translate(op.offset.x, op.offset.y, op.offset.z);
    }

    case "Rotate": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.rotate(op.angles.x, op.angles.y, op.angles.z);
    }

    case "Scale": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.scale(op.factor.x, op.factor.y, op.factor.z);
    }

    case "Sketch2D":
      return Solid.empty();

    case "Extrude": {
      const sketchNode = nodes[String(op.sketch)];
      if (!sketchNode) {
        throw new Error(`Extrude references missing node: ${op.sketch}`);
      }

      const direction = new Float64Array([
        op.direction.x,
        op.direction.y,
        op.direction.z,
      ]);

      // Handle Text2D nodes (text extrusion)
      if (sketchNode.op.type === "Text2D") {
        const textOp = sketchNode.op as Text2DOp;
        const origin = new Float64Array([textOp.origin.x, textOp.origin.y, textOp.origin.z]);
        const xDir = new Float64Array([textOp.x_dir.x, textOp.x_dir.y, textOp.x_dir.z]);
        const yDir = new Float64Array([textOp.y_dir.x, textOp.y_dir.y, textOp.y_dir.z]);

        return Solid.textExtrude(
          textOp.text,
          origin,
          xDir,
          yDir,
          direction,
          textOp.height,
          textOp.font || undefined,
          textOp.alignment || undefined,
          textOp.letter_spacing ?? undefined,
          textOp.line_spacing ?? undefined,
        );
      }

      if (sketchNode.op.type !== "Sketch2D") {
        throw new Error(`Extrude references invalid sketch node: ${op.sketch} (type=${sketchNode.op.type})`);
      }
      const profile = convertSketchToProfile(sketchNode.op);
      const profileJson = JSON.stringify(profile);
      const hasTwist = op.twist_angle !== undefined && Math.abs(op.twist_angle) > 1e-12;
      const hasScale = op.scale_end !== undefined && Math.abs(op.scale_end - 1.0) > 1e-12;
      return (hasTwist || hasScale)
        ? Solid.extrudeWithOptions(
            profileJson,
            direction,
            op.twist_angle ?? 0,
            op.scale_end ?? 1.0
          )
        : Solid.extrude(profileJson, direction);
    }

    case "Revolve": {
      const sketchNode = nodes[String(op.sketch)];
      if (!sketchNode || sketchNode.op.type !== "Sketch2D") {
        throw new Error(`Revolve references invalid sketch node: ${op.sketch}`);
      }
      const profile = convertSketchToProfile(sketchNode.op);
      const profileJson = JSON.stringify(profile);
      const axisOrigin = new Float64Array([
        op.axis_origin.x,
        op.axis_origin.y,
        op.axis_origin.z,
      ]);
      const axisDir = new Float64Array([
        op.axis_dir.x,
        op.axis_dir.y,
        op.axis_dir.z,
      ]);
      return Solid.revolve(profileJson, axisOrigin, axisDir, op.angle_deg);
    }

    case "LinearPattern": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.linearPattern(
        op.direction.x,
        op.direction.y,
        op.direction.z,
        op.count,
        op.spacing,
      );
    }

    case "CircularPattern": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.circularPattern(
        op.axis_origin.x,
        op.axis_origin.y,
        op.axis_origin.z,
        op.axis_dir.x,
        op.axis_dir.y,
        op.axis_dir.z,
        op.count,
        op.angle_deg,
      );
    }

    case "Shell": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.shell(op.thickness);
    }

    case "Fillet": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.fillet(op.radius);
    }

    case "Chamfer": {
      const child = evaluateNode(op.child, nodes, Solid, cache, depth + 1);
      return child.chamfer(op.distance);
    }

    case "Sweep": {
      const sketchNode = nodes[String(op.sketch)];
      if (!sketchNode || sketchNode.op.type !== "Sketch2D") {
        throw new Error(`Sweep references invalid sketch node: ${op.sketch}`);
      }
      const profile = convertSketchToProfile(sketchNode.op);
      const profileJson = JSON.stringify(profile);

      if (op.path.type === "Line") {
        const start = new Float64Array([
          op.path.start.x,
          op.path.start.y,
          op.path.start.z,
        ]);
        const end = new Float64Array([
          op.path.end.x,
          op.path.end.y,
          op.path.end.z,
        ]);
        return Solid.sweepLine(
          profileJson,
          start,
          end,
          op.twist_angle,
          op.scale_start,
          op.scale_end,
          op.orientation,
        );
      } else {
        return Solid.sweepHelix(
          profileJson,
          op.path.radius,
          op.path.pitch,
          op.path.height,
          op.path.turns,
          op.twist_angle,
          op.scale_start,
          op.scale_end,
          op.path_segments,
          op.arc_segments,
          op.orientation,
        );
      }
    }

    case "Loft": {
      const profiles = op.sketches.map((sketchId) => {
        const sketchNode = nodes[String(sketchId)];
        if (!sketchNode || sketchNode.op.type !== "Sketch2D") {
          throw new Error(`Loft references invalid sketch node: ${sketchId}`);
        }
        return convertSketchToProfile(sketchNode.op);
      });
      return Solid.loft(JSON.stringify(profiles), op.closed);
    }

    case "ImportedMesh":
      return Solid.empty();

    case "Text2D":
      return Solid.empty();

    case "PcbBoard":
      // PcbBoard is handled at the document level via findPcbBoard.
      return Solid.empty();
  }
}
