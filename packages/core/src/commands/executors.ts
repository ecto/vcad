import type { ExecutionResult, ExecutionDisplay, SummarySegment } from "./types.js";
import type { PlannedResponse, ToolOutcome } from "./registry.js";
import { commandRegistry } from "./registry.js";
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

/** Render a part ID as a clickable segment, falling back to last 4 chars if unknown. */
function link(id: string, docStore: DocStore): SummarySegment {
  const part = docStore.partIndex.get(id);
  return {
    type: "partLink",
    partId: id,
    name: part?.name ?? id.slice(-4),
  };
}

/** Shorthand for a text segment. */
function text(s: string): SummarySegment {
  return { type: "text", text: s };
}

/** Execute a CRUD tool by name, measuring duration. */
export function executeCrud(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  const t0 = performance.now();
  // Prefer the Rust planner when the kernel exposes plan_chat_tool.
  // Falls through to the TS path when the binding isn't loaded yet,
  // when Rust returns an outcome shape we haven't wired to a docstore
  // method, or when anything in the dispatch trips. The TS fallback is
  // the authoritative path for anything Rust doesn't yet cover
  // (sketched extrudes, booleans, transforms, etc).
  const wasmResult = tryExecuteViaWasm(tool, args, docStore);
  if (wasmResult) {
    wasmResult.duration = performance.now() - t0;
    return wasmResult;
  }
  const result = executeCrudInner(tool, args, docStore, uiStore);
  result.duration = performance.now() - t0;
  return result;
}

/**
 * Ask the Rust planner what should happen, then dispatch the returned
 * `ToolOutcome` through the existing CRDT-backed docstore methods.
 * Returns `null` when wasm isn't available or when the planner's
 * response doesn't fit into one of the four outcome shapes we know how
 * to apply — both cases fall through to the legacy TS `executeCrudInner`.
 */
function tryExecuteViaWasm(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
): ExecutionResult | null {
  // Skip reads — the TS path already handles them cheaply without any
  // mutation, and the planner would just return a no-outcome
  // PlannedResponse we'd still have to translate.
  if (tool === "read") return null;

  const docJson = JSON.stringify(docStore.document);
  const planned: PlannedResponse | null = commandRegistry.planCrud(
    tool,
    args,
    docJson,
  );
  if (!planned) return null;
  if (planned.status === "error") {
    return { status: "error", result: planned.result };
  }
  if (!planned.outcome) return null;

  try {
    return dispatchOutcome(planned.result, planned.outcome, docStore);
  } catch (e) {
    console.warn("[executors] wasm dispatch failed, falling back to TS:", e);
    return null;
  }
}

/** Route a planned `ToolOutcome` through the closest CRDT-aware
 *  docstore method. Returns an `ExecutionResult` with the ids the
 *  engine assigned. */
function dispatchOutcome(
  plannedResult: string,
  outcome: ToolOutcome,
  docStore: DocStore,
): ExecutionResult | null {
  switch (outcome.kind) {
    case "add_feature": {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const engine = (docStore as any)._crdtEngine;
      if (!engine || typeof engine.add_feature !== "function") return null;
      const result = engine.add_feature(JSON.stringify(outcome.op));
      // applyApiResult is internal; we reuse the public addFromIR
      // path that the other docstore methods use by re-reading the
      // returned document from the engine. Simpler: just trigger the
      // store to re-read via its existing reactive subscriptions.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const applyApiResult = (docStore as any)._applyApiResult;
      if (typeof applyApiResult === "function") {
        applyApiResult(result);
      }
      const createdId: string | null = result?.createdFeatureId ?? null;
      if (outcome.name && createdId && typeof docStore.renamePart === "function") {
        docStore.renamePart(createdId, outcome.name);
      }
      return {
        status: "success",
        result: createdId ? `${plannedResult} with id: ${createdId}` : plannedResult,
        partId: createdId ?? undefined,
        nodeId: createdId ?? undefined,
      };
    }
    case "remove_part": {
      docStore.removePart(outcome.part_id);
      return { status: "success", result: plannedResult };
    }
    case "set_part_material": {
      docStore.setPartMaterial(outcome.part_id, outcome.material);
      return { status: "success", result: plannedResult, partId: outcome.part_id };
    }
    case "update_params": {
      for (const [key, value] of Object.entries(outcome.params)) {
        if (key === "type") continue;
        docStore.setFeatureParam(outcome.node_id, key, value as never);
      }
      return { status: "success", result: plannedResult, nodeId: outcome.node_id };
    }
  }
}

function executeCrudInner(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  switch (tool) {
    case "create":
      return executeCreateWithName(
        args.type as string,
        args.params as Record<string, unknown>,
        args.parent_part_id as string | undefined,
        args.name as string | undefined,
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
    case "set_material":
      return executeSetMaterial(args.part_id as string, args.material as string, docStore);
    default:
      return { status: "error", result: `Unknown tool: ${tool}` };
  }
}

function executeSetMaterial(
  partId: string,
  materialKey: string,
  docStore: DocStore,
): ExecutionResult {
  if (!partId) return { status: "error", result: "set_material requires part_id" };
  if (!materialKey) return { status: "error", result: "set_material requires material key" };
  const err = validatePartId(partId, docStore, "set_material part_id");
  if (err) return err;
  try {
    docStore.setPartMaterial(partId, materialKey);
    return {
      status: "success",
      result: `Set ${partId} material to ${materialKey}`,
      partId,
      display: {
        summary: [
          text("⬤ Material "),
          link(partId, docStore),
          text(` = ${materialKey}`),
        ],
        fields: [{ label: "material", value: materialKey }],
        affectedPartIds: [partId],
      },
    };
  } catch (e) {
    return { status: "error", result: e instanceof Error ? e.message : "set_material failed" };
  }
}

/** Apply a user-provided name to a freshly created part, if any. */
function applyName(docStore: DocStore, partId: string | null | undefined, name: string | undefined): void {
  if (!partId || !name) return;
  if (typeof docStore.renamePart !== "function") return;
  try {
    docStore.renamePart(partId, name);
  } catch {
    // non-fatal: rename failures shouldn't break the create call
  }
}

/** Wrap executeCreate with an optional post-rename that keeps display segments in sync. */
function executeCreateWithName(
  type: string,
  params: Record<string, unknown>,
  parentPartId: string | undefined,
  name: string | undefined,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  const result = executeCreate(type, params, parentPartId, docStore, uiStore);
  if (result.status === "success" && name && result.partId) {
    applyName(docStore, result.partId, name);
    // Patch any partLink segments in the display that reference the newly named part
    if (result.display?.summary) {
      for (const seg of result.display.summary) {
        if (seg.type === "partLink" && seg.partId === result.partId) {
          seg.name = name;
        }
      }
    }
  }
  return result;
}

/** Tool-name aliases — Claude occasionally hallucinates extra underscores in
 * snake_case (`sketch_2_d` instead of `sketch_2d`). Normalize at the boundary
 * so legitimate prompts don't fail just because the model picked the wrong
 * shape of underscore. */
const CREATE_TYPE_ALIASES: Record<string, string> = {
  sketch_2_d: "sketch_2d",
  text_2_d: "text_2d",
};

function executeCreate(
  type: string,
  params: Record<string, unknown>,
  parentPartId: string | undefined,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  type = CREATE_TYPE_ALIASES[type] ?? type;
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

        const fields: Array<{ label: string; value: string }> = [];
        if (type === "cube" && params.size) {
          const s = params.size as { x: number; y: number; z: number };
          fields.push({ label: "size", value: `${s.x}×${s.y}×${s.z} mm` });
        }
        if (type === "cylinder") {
          if (params.radius != null) fields.push({ label: "radius", value: `${params.radius} mm` });
          if (params.height != null) fields.push({ label: "height", value: `${params.height} mm` });
        }
        if (type === "sphere" && params.radius != null) {
          fields.push({ label: "radius", value: `${params.radius} mm` });
        }

        const sizeSuffix = fields.length > 0 ? ` ${fields.map((f) => f.value).join(", ")}` : "";
        const capitalized = type.charAt(0).toUpperCase() + type.slice(1);

        return {
          status: "success",
          result: `Created ${type} with id: ${partId}`,
          partId,
          display: {
            summary: [text(`+ ${capitalized}${sizeSuffix} `), link(partId, docStore)],
            fields,
            affectedPartIds: [partId],
          },
        };
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
        return {
          status: "success",
          result: `Translated ${child} by (${offset.x}, ${offset.y}, ${offset.z})`,
          partId: child,
          display: {
            summary: [
              text("↦ Translate "),
              link(child, docStore),
              text(` by (${offset.x}, ${offset.y}, ${offset.z})`),
            ],
            fields: [{ label: "offset", value: `(${offset.x}, ${offset.y}, ${offset.z}) mm` }],
            affectedPartIds: [child],
          },
        };
      }
      case "rotate": {
        const child = params.child as string;
        const angles = params.angles as { x: number; y: number; z: number };
        if (!child || !angles) return { status: "error", result: "rotate requires child and angles" };
        const err = validatePartId(child, docStore, "rotate child");
        if (err) return err;
        docStore.setRotation(child, angles);
        return {
          status: "success",
          result: `Rotated ${child}`,
          partId: child,
          display: {
            summary: [
              text("↻ Rotate "),
              link(child, docStore),
              text(` by (${angles.x}°, ${angles.y}°, ${angles.z}°)`),
            ],
            fields: [{ label: "angles", value: `(${angles.x}°, ${angles.y}°, ${angles.z}°)` }],
            affectedPartIds: [child],
          },
        };
      }
      case "scale": {
        const child = params.child as string;
        const factor = params.factor as { x: number; y: number; z: number };
        if (!child || !factor) return { status: "error", result: "scale requires child and factor" };
        const err = validatePartId(child, docStore, "scale child");
        if (err) return err;
        docStore.setScale(child, factor);
        return {
          status: "success",
          result: `Scaled ${child}`,
          partId: child,
          display: {
            summary: [
              text("⇱ Scale "),
              link(child, docStore),
              text(` by (${factor.x}, ${factor.y}, ${factor.z})`),
            ],
            fields: [{ label: "factor", value: `(${factor.x}, ${factor.y}, ${factor.z})` }],
            affectedPartIds: [child],
          },
        };
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
        if (!resultId) return { status: "error", result: `${type} failed` };
        const verb = type === "union" ? "Join" : type === "difference" ? "Cut" : "Intersect";
        const icon = type === "union" ? "⊕" : type === "difference" ? "⊖" : "⊗";
        return {
          status: "success",
          result: `Applied ${type} → new part id: ${resultId}`,
          partId: resultId,
          display: {
            summary: [
              text(`${icon} ${verb} `),
              link(left, docStore),
              text(" with "),
              link(right, docStore),
              text(" → "),
              link(resultId, docStore),
            ],
            fields: [{ label: "operation", value: type }],
            affectedPartIds: [left, right, resultId],
          },
        };
      }

      case "fillet": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "fillet requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "fillet target");
        if (err) return err;
        const id = docStore.addFillet(target, params.radius as number);
        if (!id) return { status: "error", result: "Fillet failed — target may not be a solid" };
        return {
          status: "success",
          result: `Applied ${params.radius}mm fillet to ${target} → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`⌒ Fillet `),
              link(target, docStore),
              text(` r=${params.radius}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "radius", value: `${params.radius} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
      case "chamfer": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "chamfer requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "chamfer target");
        if (err) return err;
        const id = docStore.addChamfer(target, params.distance as number);
        if (!id) return { status: "error", result: "Chamfer failed — target may not be a solid" };
        return {
          status: "success",
          result: `Applied ${params.distance}mm chamfer to ${target} → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`⌐ Chamfer `),
              link(target, docStore),
              text(` d=${params.distance}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "distance", value: `${params.distance} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
      case "shell": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "shell requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "shell target");
        if (err) return err;
        const id = docStore.addShell(target, params.thickness as number);
        if (!id) return { status: "error", result: "Shell failed — target may not be a solid" };
        return {
          status: "success",
          result: `Shelled ${target} with ${params.thickness}mm walls → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`□ Shell `),
              link(target, docStore),
              text(` t=${params.thickness}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "thickness", value: `${params.thickness} mm` }],
            affectedPartIds: [target, id],
          },
        };
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
          if (!partId) {
            return { status: "error", result: "Extrude failed — check sketch segments form a closed loop" };
          }
          const depth = Math.sqrt(direction.x ** 2 + direction.y ** 2 + direction.z ** 2);
          return {
            status: "success",
            result: `Extruded sketch → new part id: ${partId}`,
            partId,
            display: {
              summary: [
                text(`▲ Extrude sketch (${s.segments.length} segs, ${depth.toFixed(1)}mm) → `),
                link(partId, docStore),
              ],
              fields: [
                { label: "segments", value: `${s.segments.length}` },
                { label: "depth", value: `${depth.toFixed(2)} mm` },
                { label: "origin", value: `(${s.origin.x}, ${s.origin.y}, ${s.origin.z})` },
              ],
              affectedPartIds: [partId],
            },
          };
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
    const part = docStore.partIndex.get(partId);
    const name = part?.name ?? partId.slice(-4);
    docStore.removePart(partId);
    uiStore.clearSelection();
    return {
      status: "success",
      result: `Deleted part ${partId}`,
      display: {
        summary: [
          text("✕ Delete "),
          { type: "partLink", partId, name },
        ],
        fields: [{ label: "part id", value: partId }],
        affectedPartIds: [partId],
      },
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Delete failed" };
  }
}
