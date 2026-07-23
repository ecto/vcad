/**
 * Document-level design-constraint tools — KiCad-v11-class geometric
 * constraints, generalized to the whole vcad document.
 *
 *  - `add_constraint` — batch-add constraints (footprints, board outline,
 *    sketch points, named part edges) and auto-solve.
 *  - `delete_constraint` / `list_constraints` — manage the persisted set.
 *  - `solve_constraints` — explicit re-solve (after set_placement /
 *    set_board_outline / geometry edits).
 *
 * Dimensions are expressions: a distance can be `"board_width - 2*margin"`
 * over named document parameters — `set_parameters` re-solves the set.
 * Mechanical part anchors are authoritative: constraints pull boards and
 * sketches toward parts, never the reverse. The solver never touches
 * copper; when solved footprints have routed traces, re-run `route_nets`.
 */

import { behavior, type ToolDef } from "./tool-def.js";
import type { Anchor, ConstraintKind, DesignConstraint, Document } from "@vcad/ir";
import {
  checkDesignConstraints,
  solveDesignConstraints,
  type DesignSolveReport,
} from "@vcad/engine";
import { resolveDocInput } from "./session-core.js";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import { summarizePlacementDrc } from "./ecad.js";

type ToolResult = {
  content: Array<{ type: "text"; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
};

function jsonResult(payload: unknown): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}

function err(text: string): ToolResult {
  return { content: [{ type: "text", text: `Error: ${text}` }], isError: true };
}

const anchorDescription =
  "An anchor: {kind:'pcbFootprint', node, ref, pad?} (footprint origin or " +
  "named pad), {kind:'pcbOutlineVertex'|'pcbOutlineEdge', node, index}, " +
  "{kind:'sketchPoint', node, segment, point:'start'|'end'|'center'}, or " +
  "{kind:'partEdge', node, faceA, faceB} (topological face names, e.g. " +
  "'n7:top'/'n7:front' — resolved fail-closed; part geometry is " +
  "authoritative and never moves).";

export const addConstraintSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const, description: "Session id." },
    constraints: {
      type: "array" as const,
      description:
        "Constraints to add. Each entry is a tagged kind object plus optional " +
        "`label` (receipt claim name) and `driven` (reference dimension: " +
        "measured, not enforced). Kinds: coincident{a,b}, distance{a,b,value}, " +
        "horizontalDistance{a,value}, verticalDistance{a,value}, " +
        "horizontal{a,b}, vertical{a,b}, parallel{a,b}, perpendicular{a,b}, " +
        "equalLength{a,b}, length{a,value}, pointOnEdge{point,edge}, " +
        "concentric{a,b}, fixed{a}, rotation{node,ref,value}, " +
        "symmetric{a,b,axis}, angle{a,b,value}. Dimensional `value` is a " +
        "number (mm/deg) or a formula string over document parameters " +
        "(\"board_width - 2*margin\"). " +
        anchorDescription,
      items: { type: "object" as const },
    },
  },
  required: ["document_id", "constraints"],
};

export const deleteConstraintSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const, description: "Session id." },
    id: { type: "string" as const, description: "Constraint id (or label) to delete." },
    all: { type: "boolean" as const, description: "Delete every constraint." },
  },
  required: ["document_id"],
};

export const listConstraintsSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const, description: "Session id." },
    document: { type: "object" as const, description: "Inline Document IR (stateless path)." },
  },
};

export const solveConstraintsSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const, description: "Session id." },
  },
  required: ["document_id"],
};

/** Next free "cN" id given the existing set. */
function nextId(existing: DesignConstraint[]): number {
  let max = 0;
  for (const c of existing) {
    const m = /^c(\d+)$/.exec(c.id);
    if (m) max = Math.max(max, Number(m[1]));
  }
  return max + 1;
}

/** Apply a solved document back onto the session object in place. */
function applySolved(doc: Document, solved: Document): void {
  doc.nodes = solved.nodes;
  doc.constraints = solved.constraints;
}

/** Post-solve caveats: routed copper under moved footprints. */
function copperWarnings(doc: Document, report: DesignSolveReport): string[] {
  if (report.movedFootprints.length === 0) return [];
  for (const nodeId of getPcbNodeIds(doc)) {
    const pcb = getNodePcb(doc, nodeId);
    if (pcb && (pcb.traces?.length || pcb.vias?.length)) {
      return [
        `moved footprints (${report.movedFootprints.join(", ")}) may have routed copper — re-run route_nets`,
      ];
    }
  }
  return [];
}

/** Placement DRC for the first board when footprints moved. */
async function placementDrcIfMoved(doc: Document, report: DesignSolveReport) {
  if (report.movedFootprints.length === 0) return undefined;
  const nodeId = getPcbNodeIds(doc)[0];
  const pcb = nodeId != null ? getNodePcb(doc, nodeId) : null;
  return pcb ? await summarizePlacementDrc(pcb) : undefined;
}

async function solveAndReport(doc: Document, documentId?: string): Promise<ToolResult> {
  const outcome = await solveDesignConstraints(doc);
  if (outcome.status !== "ok") {
    return err(`constraint solve unavailable: ${outcome.reason}`);
  }
  const { document: solved, report } = outcome.value;
  applySolved(doc, solved);
  const placementDrc = await placementDrcIfMoved(doc, report);
  const warnings = [...report.warnings, ...copperWarnings(doc, report)];
  return jsonResult({
    success: report.converged && report.errors.length === 0,
    report: { ...report, warnings },
    ...(placementDrc ? { placement_drc: placementDrc } : {}),
    ...(documentId ? { document_id: documentId } : {}),
  });
}

/** Split a wire entry into (kind, label, driven), validating the tag. */
function parseEntry(
  entry: Record<string, unknown>,
): { kind: ConstraintKind; label?: string; driven: boolean } | string {
  const { label, driven, id: _ignored, ...kindFields } = entry;
  const type = kindFields.type;
  if (typeof type !== "string") return "each constraint needs a `type` field";
  const known = [
    "coincident",
    "distance",
    "horizontalDistance",
    "verticalDistance",
    "horizontal",
    "vertical",
    "parallel",
    "perpendicular",
    "equalLength",
    "length",
    "pointOnEdge",
    "concentric",
    "fixed",
    "rotation",
    "symmetric",
    "angle",
  ];
  if (!known.includes(type)) return `unknown constraint type "${type}"`;
  return {
    kind: kindFields as unknown as ConstraintKind,
    label: typeof label === "string" ? label : undefined,
    driven: driven === true,
  };
}

/** Validate anchors against the document so bad refs fail at add time. */
function validateAnchors(doc: Document, kind: ConstraintKind): string | undefined {
  const anchors: Anchor[] = [];
  for (const v of Object.values(kind as unknown as Record<string, unknown>)) {
    if (v && typeof v === "object" && "kind" in (v as Record<string, unknown>)) {
      anchors.push(v as Anchor);
    }
  }
  for (const a of anchors) {
    const node = doc.nodes[String((a as { node: number }).node)];
    if (!node) return `anchor references missing node ${(a as { node: number }).node}`;
    if (a.kind === "pcbFootprint") {
      const pcb = getNodePcb(doc, (a as { node: number }).node);
      if (!pcb) return `node ${(a as { node: number }).node} is not a PCB board`;
      const fp = pcb.footprints.find((f) => f.ref === (a as { ref: string }).ref);
      if (!fp) return `footprint "${(a as { ref: string }).ref}" not found`;
    }
    if (a.kind === "pcbOutlineVertex" || a.kind === "pcbOutlineEdge") {
      const pcb = getNodePcb(doc, (a as { node: number }).node);
      if (!pcb) return `node ${(a as { node: number }).node} is not a PCB board`;
      const n = pcb.outline.vertices?.length ?? 0;
      if ((a as { index: number }).index >= n) {
        return `outline index ${(a as { index: number }).index} out of range (${n} vertices)`;
      }
    }
  }
  return undefined;
}

export async function addConstraint(args: Record<string, unknown>): Promise<ToolResult> {
  const ctx = resolveDocInput(args);
  const entries = Array.isArray(args.constraints)
    ? (args.constraints as Array<Record<string, unknown>>)
    : undefined;
  if (!entries || entries.length === 0) {
    return err("`constraints` must be a non-empty array of constraint objects");
  }
  ctx.doc.constraints ??= [];
  let id = nextId(ctx.doc.constraints);
  const added: string[] = [];
  for (const entry of entries) {
    const parsed = parseEntry(entry);
    if (typeof parsed === "string") return err(parsed);
    const bad = validateAnchors(ctx.doc, parsed.kind);
    if (bad) return err(`constraint ${added.length + 1}: ${bad}`);
    const cid = `c${id++}`;
    ctx.doc.constraints.push({
      id: cid,
      ...(parsed.label !== undefined ? { label: parsed.label } : {}),
      kind: parsed.kind,
      driven: parsed.driven,
    } as DesignConstraint);
    added.push(cid);
  }
  const result = await solveAndReport(ctx.doc, ctx.documentId);
  if (result.isError) return result;
  const payload = JSON.parse(result.content[0].text) as Record<string, unknown>;
  return jsonResult({ added, ...payload });
}

export async function deleteConstraint(args: Record<string, unknown>): Promise<ToolResult> {
  const ctx = resolveDocInput(args);
  const constraints = ctx.doc.constraints ?? [];
  if (args.all === true) {
    const removed = constraints.length;
    ctx.doc.constraints = [];
    return jsonResult({ removed, ...(ctx.documentId ? { document_id: ctx.documentId } : {}) });
  }
  const id = typeof args.id === "string" ? args.id : "";
  if (!id) return err("pass `id` (constraint id or label) or `all: true`");
  const before = constraints.length;
  ctx.doc.constraints = constraints.filter((c) => c.id !== id && c.label !== id);
  const removed = before - ctx.doc.constraints.length;
  if (removed === 0) {
    return err(`no constraint with id or label "${id}" — see list_constraints`);
  }
  // Deleting only frees DOF; no re-solve needed.
  return jsonResult({ removed, ...(ctx.documentId ? { document_id: ctx.documentId } : {}) });
}

export async function listConstraints(args: Record<string, unknown>): Promise<ToolResult> {
  const ctx = resolveDocInput(args);
  const constraints = ctx.doc.constraints ?? [];
  if (constraints.length === 0) {
    return jsonResult({
      constraints: [],
      hint: "add_constraint persists solver-enforced geometric relationships",
    });
  }
  const outcome = await checkDesignConstraints(ctx.doc);
  const report = outcome.status === "ok" ? outcome.value : undefined;
  const residuals = new Map(report?.residuals.map((r) => [r.id, r.residual]) ?? []);
  const measured = new Map(report?.drivenValues.map((d) => [d.id, d.value]) ?? []);
  return jsonResult({
    constraints: constraints.map((c) => ({
      ...c,
      ...(measured.has(c.id) ? { measured: measured.get(c.id) } : {}),
      ...(residuals.has(c.id) ? { residual: residuals.get(c.id) } : {}),
    })),
    ...(report
      ? {
          groups: report.groups,
          converged: report.converged,
          errors: report.errors,
        }
      : { check: "unavailable — kernel WASM not loaded" }),
    ...(ctx.documentId ? { document_id: ctx.documentId } : {}),
  });
}

export async function solveConstraints(args: Record<string, unknown>): Promise<ToolResult> {
  const ctx = resolveDocInput(args);
  if (!ctx.doc.constraints || ctx.doc.constraints.length === 0) {
    return err("document has no constraints — add some with add_constraint");
  }
  return solveAndReport(ctx.doc, ctx.documentId);
}

export const toolDefs: ToolDef[] = [
  {
    name: "add_constraint",
    pack: null,
    description:
      "Add solver-enforced geometric constraints to the document and solve " +
      "immediately — KiCad-v11-class constraints generalized to the whole " +
      "design: PCB footprints and board outline, sketch points, and named " +
      "mechanical part edges (cross-domain: 'USB connector coincident with " +
      "the enclosure cutout'). Dimensional values accept formulas over " +
      "document parameters; set_parameters re-solves. `driven: true` makes a " +
      "reference dimension (measured, back-annotated, never enforced). " +
      "Constraints persist and re-verify via build_receipt / verify_receipt " +
      "as constraint.* claims (Holds/Stale/Violated). The solver moves " +
      "footprints, outline vertices, and sketch points — never copper or " +
      "part geometry; re-run route_nets when moved footprints were routed.",
    inputSchema: addConstraintSchema,
    handler: (a) => addConstraint(a),
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "delete_constraint",
    pack: null,
    description:
      "Delete a design constraint by id or label (or `all: true` to clear " +
      "the set). Frees degrees of freedom; geometry stays where it is.",
    inputSchema: deleteConstraintSchema,
    handler: (a) => deleteConstraint(a),
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "list_constraints",
    pack: null,
    description:
      "List the document's design constraints with their current measured " +
      "values and residuals, per-group degrees of freedom, and " +
      "over/under-constrained status. Use before deleting or when a solve " +
      "reports non-convergence.",
    inputSchema: listConstraintsSchema,
    handler: (a) => listConstraints(a),
    behavior: behavior({}),
  },
  {
    name: "solve_constraints",
    pack: null,
    description:
      "Re-solve the document's design constraints against current geometry " +
      "— run after set_placement, set_board_outline, or sketch edits that " +
      "may have violated them. Reports per-group convergence and DOF, moved " +
      "footprints/vertices/sketches, and back-annotates driven dimensions.",
    inputSchema: solveConstraintsSchema,
    handler: (a) => solveConstraints(a),
    behavior: behavior({ writesDoc: true }),
  },
];
