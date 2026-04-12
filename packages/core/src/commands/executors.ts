import type { ExecutionResult } from "./types.js";
import { useDocumentStore } from "../stores/document-store.js";
import { useUiStore } from "../stores/ui-store.js";
import { vec3Cross, vec3Normalize } from "@vcad/ir";

type DocStore = ReturnType<typeof useDocumentStore.getState>;
type UiStore = ReturnType<typeof useUiStore.getState>;

/** Validate that a part ID exists in the document. */
function validatePartId(partId: string, docStore: DocStore, label: string): ExecutionResult | null {
  if (!docStore.partIndex.get(partId)) {
    const available = docStore.parts.map((p) => p.id).slice(0, 10).join(", ");
    return {
      status: "error",
      result: `${label} "${partId}" not found. Available parts: [${available}]${docStore.parts.length > 10 ? ` (+${docStore.parts.length - 10} more)` : ""}`,
    };
  }
  return null;
}

type Vec2Lite = { x: number; y: number };
type SketchSegLite = {
  type: "Line" | "Arc";
  start: Vec2Lite;
  end: Vec2Lite;
  center?: Vec2Lite;
  ccw?: boolean;
};

/** Validate a sketch forms a closed loop with matched segment endpoints.
 *  Returns an error result or null if valid. */
function validateSketch(segments: unknown[]): ExecutionResult | null {
  if (!Array.isArray(segments) || segments.length === 0) {
    return { status: "error", result: "sketch must have at least one segment" };
  }
  if (segments.length < 2) {
    return {
      status: "error",
      result: "sketch must have at least 2 segments to form a closed loop. A single arc is not a closed profile — add a line or another arc to close it.",
    };
  }

  const segs = segments as SketchSegLite[];
  const eps = 1e-3; // 1 micron
  const matches = (a: Vec2Lite, b: Vec2Lite) =>
    Math.abs(a.x - b.x) < eps && Math.abs(a.y - b.y) < eps;

  for (let i = 0; i < segs.length; i++) {
    const curr = segs[i]!;
    const next = segs[(i + 1) % segs.length]!;
    if (!curr.start || !curr.end || !next.start) {
      return { status: "error", result: `segment ${i} missing start/end points` };
    }
    if (!matches(curr.end, next.start)) {
      return {
        status: "error",
        result: `sketch is not closed: segment ${i} ends at (${curr.end.x}, ${curr.end.y}) but segment ${(i + 1) % segs.length} starts at (${next.start.x}, ${next.start.y}). Each segment's end must match the next segment's start.`,
      };
    }
  }
  return null;
}

/** Execute a CRUD tool by name. */
export function executeCrud(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  switch (tool) {
    case "create":
      return executeCreate(
        args.type as string,
        args.params as Record<string, unknown>,
        args.parent_part_id as string | undefined,
        docStore,
        uiStore,
      );
    case "read":
      return executeRead(args.part_id as string | undefined, docStore);
    case "update":
      return executeUpdate(
        args.node_id as string,
        args.params as Record<string, unknown>,
        docStore,
      );
    case "delete":
      return executeDelete(args.part_id as string, docStore, uiStore);
    default:
      return { status: "error", result: `Unknown tool: ${tool}` };
  }
}

function executeCreate(
  type: string,
  params: Record<string, unknown>,
  parentPartId: string | undefined,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  try {
    switch (type) {
      case "cube":
      case "cylinder":
      case "sphere": {
        const partId = docStore.addPrimitive(type as "cube" | "cylinder" | "sphere");
        if (params && Object.keys(params).length > 0) {
          // Defer param update to next microtask to avoid WASM re-entrant borrow.
          // addPrimitive's set() triggers subscribers synchronously — calling
          // setFeatureParam in the same tick would re-enter the CRDT engine.
          const capitalizedType = type.charAt(0).toUpperCase() + type.slice(1);
          setTimeout(() => {
            docStore.updatePrimitiveOp(partId, { type: capitalizedType, ...params });
          }, 0);
        }
        uiStore.select(partId);
        return { status: "success", result: `Created ${type} with id: ${partId}`, partId };
      }

      case "cone":
        return { status: "error", result: "Cone primitive not yet supported via addPrimitive" };

      case "translate": {
        const child = params.child as string;
        const offset = params.offset as { x: number; y: number; z: number };
        if (!child || !offset) return { status: "error", result: "translate requires child and offset" };
        const err = validatePartId(child, docStore, "translate child");
        if (err) return err;
        docStore.setTranslation(child, offset);
        return { status: "success", result: `Translated ${child} by (${offset.x}, ${offset.y}, ${offset.z})`, partId: child };
      }
      case "rotate": {
        const child = params.child as string;
        const angles = params.angles as { x: number; y: number; z: number };
        if (!child || !angles) return { status: "error", result: "rotate requires child and angles" };
        const err = validatePartId(child, docStore, "rotate child");
        if (err) return err;
        docStore.setRotation(child, angles);
        return { status: "success", result: `Rotated ${child}`, partId: child };
      }
      case "scale": {
        const child = params.child as string;
        const factor = params.factor as { x: number; y: number; z: number };
        if (!child || !factor) return { status: "error", result: "scale requires child and factor" };
        const err = validatePartId(child, docStore, "scale child");
        if (err) return err;
        docStore.setScale(child, factor);
        return { status: "success", result: `Scaled ${child}`, partId: child };
      }

      case "union":
      case "difference":
      case "intersection": {
        let left = params.left as string;
        let right = params.right as string;
        if (!left || !right) {
          const selectedIds = Array.from(uiStore.selectedPartIds);
          if (selectedIds.length !== 2) {
            return { status: "error", result: "Boolean requires left and right part IDs, or exactly 2 parts selected" };
          }
          left = selectedIds[0]!;
          right = selectedIds[1]!;
        }
        const lerr = validatePartId(left, docStore, "boolean left");
        if (lerr) return lerr;
        const rerr = validatePartId(right, docStore, "boolean right");
        if (rerr) return rerr;
        const resultId = docStore.applyBoolean(type, left, right);
        return resultId
          ? { status: "success", result: `Applied ${type} → new part id: ${resultId}`, partId: resultId }
          : { status: "error", result: `${type} failed` };
      }

      case "fillet": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "fillet requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "fillet target");
        if (err) return err;
        const id = docStore.addFillet(target, params.radius as number);
        return id
          ? { status: "success", result: `Applied ${params.radius}mm fillet to ${target} → new part id: ${id}`, partId: id }
          : { status: "error", result: "Fillet failed — target may not be a solid" };
      }
      case "chamfer": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "chamfer requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "chamfer target");
        if (err) return err;
        const id = docStore.addChamfer(target, params.distance as number);
        return id
          ? { status: "success", result: `Applied ${params.distance}mm chamfer to ${target} → new part id: ${id}`, partId: id }
          : { status: "error", result: "Chamfer failed — target may not be a solid" };
      }
      case "shell": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "shell requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "shell target");
        if (err) return err;
        const id = docStore.addShell(target, params.thickness as number);
        return id
          ? { status: "success", result: `Shelled ${target} with ${params.thickness}mm walls → new part id: ${id}`, partId: id }
          : { status: "error", result: "Shell failed — target may not be a solid" };
      }

      case "extrude": {
        const sketch = params.sketch;
        if (!sketch) return { status: "error", result: "extrude requires a sketch parameter" };
        const direction = params.direction as { x: number; y: number; z: number };
        if (!direction) return { status: "error", result: "extrude requires direction" };

        if (typeof sketch === "object") {
          const s = sketch as {
            origin: { x: number; y: number; z: number };
            x_dir: { x: number; y: number; z: number };
            y_dir: { x: number; y: number; z: number };
            segments: unknown[];
          };
          const sketchErr = validateSketch(s.segments);
          if (sketchErr) return sketchErr;
          const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
          const plane = { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal };
          const partId = docStore.addExtrude(
            plane, s.origin, s.segments as never[], direction,
            { twist_angle: params.twist_angle as number | undefined, scale_end: params.scale_end as number | undefined },
          );
          return partId
            ? { status: "success", result: `Extruded sketch → new part id: ${partId}`, partId }
            : { status: "error", result: "Extrude failed — check sketch segments form a closed loop" };
        }
        return { status: "error", result: "Extrude from existing sketch node not yet supported" };
      }

      case "revolve": {
        const sketch = params.sketch;
        if (!sketch || typeof sketch !== "object") return { status: "error", result: "revolve requires an inline sketch" };
        const s = sketch as {
          origin: { x: number; y: number; z: number };
          x_dir: { x: number; y: number; z: number };
          y_dir: { x: number; y: number; z: number };
          segments: unknown[];
        };
        const axisOrigin = params.axis_origin as { x: number; y: number; z: number };
        const axisDir = params.axis_dir as { x: number; y: number; z: number };
        const sketchErr = validateSketch(s.segments);
        if (sketchErr) return sketchErr;
        const angleDeg = params.angle_deg as number;
        const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
        const plane = { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal };
        const partId = docStore.addRevolve(plane, s.origin, s.segments as never[], axisOrigin, axisDir, angleDeg);
        return partId
          ? { status: "success", result: `Revolved sketch → new part id: ${partId}`, partId }
          : { status: "error", result: "Revolve failed" };
      }

      case "sweep": {
        const sketch = params.sketch;
        if (!sketch || typeof sketch !== "object") return { status: "error", result: "sweep requires an inline sketch" };
        const s = sketch as {
          origin: { x: number; y: number; z: number };
          x_dir: { x: number; y: number; z: number };
          y_dir: { x: number; y: number; z: number };
          segments: unknown[];
        };
        const sketchErr = validateSketch(s.segments);
        if (sketchErr) return sketchErr;
        const path = params.path as Record<string, unknown>;
        const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
        const plane = { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal };
        const partId = docStore.addSweep(plane, s.origin, s.segments as never[], path as never, {
          twist_angle: params.twist_angle as number | undefined,
          scale_start: params.scale_start as number | undefined,
          scale_end: params.scale_end as number | undefined,
        });
        return partId
          ? { status: "success", result: `Swept sketch → new part id: ${partId}`, partId }
          : { status: "error", result: "Sweep failed" };
      }

      case "loft": {
        const sketches = params.sketches as Array<{
          origin: { x: number; y: number; z: number };
          x_dir: { x: number; y: number; z: number };
          y_dir: { x: number; y: number; z: number };
          segments: unknown[];
        }>;
        if (!sketches || sketches.length < 2) return { status: "error", result: "loft requires at least 2 sketch profiles" };
        const profiles = sketches.map((s) => {
          const normal = vec3Normalize(vec3Cross(s.x_dir, s.y_dir));
          return {
            plane: { type: "face" as const, origin: s.origin, xDir: s.x_dir, yDir: s.y_dir, normal },
            origin: s.origin,
            segments: s.segments as never[],
          };
        });
        const partId = docStore.addLoft(profiles, { closed: params.closed as boolean | undefined });
        return partId
          ? { status: "success", result: `Lofted ${sketches.length} profiles`, partId }
          : { status: "error", result: "Loft failed" };
      }

      case "sketch_2d":
        return { status: "success", result: "Sketch2D is typically used inline with extrude/revolve. Use create(type: 'extrude', ...) with an inline sketch instead." };

      default:
        return { status: "error", result: `Unknown create type: ${type}` };
    }
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Create failed" };
  }
}

function executeRead(
  partId: string | undefined,
  docStore: DocStore,
): ExecutionResult {
  try {
    if (!partId) {
      const parts = docStore.parts.map((p) => ({
        id: p.id,
        name: p.name,
        kind: p.kind,
      }));
      return { status: "success", result: JSON.stringify(parts) };
    }

    const part = docStore.partIndex.get(partId);
    if (!part) return { status: "error", result: `Part ${partId} not found` };

    const doc = docStore.document;
    const nodes: Array<{ nodeId: string; type: string; params: Record<string, unknown> }> = [];

    const walkNode = (nodeId: string | number) => {
      const node = doc.nodes[String(nodeId)];
      if (!node) return;
      const op = node.op;
      const opType = (op as { type?: string }).type;
      if (opType) {
        const params = { ...op } as Record<string, unknown>;
        delete params.type;
        nodes.push({ nodeId: String(nodeId), type: opType, params });
      }
    };

    // All PartInfo variants have translateNodeId as the outermost node
    const p = part as { translateNodeId?: number | string };
    if (p.translateNodeId !== undefined) {
      walkNode(p.translateNodeId);
    }

    return {
      status: "success",
      result: JSON.stringify({
        id: part.id,
        name: part.name,
        kind: part.kind,
        nodes,
      }),
      partId,
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Read failed" };
  }
}

function executeUpdate(
  nodeId: string,
  params: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult {
  try {
    let targetPartId: string | undefined;
    for (const part of docStore.parts) {
      if (docStore.document.nodes[nodeId]) {
        targetPartId = part.id;
        break;
      }
    }

    if (!targetPartId) {
      return { status: "error", result: `Node ${nodeId} not found in any part` };
    }

    for (const [key, value] of Object.entries(params)) {
      let crdtValue: unknown;
      if (typeof value === "number") {
        crdtValue = { F64: value };
      } else if (typeof value === "boolean") {
        crdtValue = { Bool: value };
      } else if (typeof value === "string") {
        crdtValue = { String: value };
      } else if (typeof value === "object" && value !== null && "x" in value && "y" in value && "z" in value) {
        const v = value as { x: number; y: number; z: number };
        crdtValue = { Vec3: [v.x, v.y, v.z] };
      } else {
        continue;
      }

      docStore.setFeatureParam(targetPartId, key, crdtValue as never);
    }

    return {
      status: "success",
      result: `Updated node ${nodeId}: ${Object.keys(params).join(", ")}`,
      partId: targetPartId,
      nodeId,
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Update failed" };
  }
}

function executeDelete(
  partId: string,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  try {
    docStore.removePart(partId);
    uiStore.clearSelection();
    return { status: "success", result: `Deleted part ${partId}` };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Delete failed" };
  }
}
