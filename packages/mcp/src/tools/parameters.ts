/**
 * Parametric-parameter tools — the differentiable seam over MCP.
 *
 *  - `list_parameters` reads a document's named parameters (raw expression,
 *    resolved value, unit, scrub bounds).
 *  - `set_parameters` batch-updates parameter values and reports a `changed`
 *    diff of the deltas. A document writer: goes through the standard session
 *    pipeline (undo snapshot, integrity, durable persist) via the dispatch
 *    layer.
 *  - `parameter_gradient` differentiates the mass-property + bounding-box
 *    QoIs with respect to one named parameter (`d QoI / dθ`) through the
 *    Rust differentiable seam — a capability no other CAD MCP exposes.
 *
 * `optimize_parameters` (solve params for a target QoI) is deferred: the
 * kernel ships an L-BFGS optimizer, but wiring a *document* parameter into it
 * (build closure + seeding synthesis + objective) is new Rust API design.
 * See the follow-up issue.
 */

import type { Engine, PartParameterGradient } from "@vcad/engine";
import { behavior, type ToolDef } from "./tool-def.js";
import { resolveParameters, solveDesignConstraints } from "@vcad/engine";
import type { Document, Expr, Parameter } from "@vcad/ir";
import { appendIntegrity, computeIntegrity } from "./integrity.js";
import {
  getSession,
  resolveDocInput,
  recordTriangles,
} from "./session-core.js";

/** MCP result shape used across these tools. */
type ToolResult = {
  content: Array<{ type: "text"; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
};

function jsonResult(payload: unknown): ToolResult {
  return {
    content: [{ type: "text", text: JSON.stringify(payload, null, 2) }],
  };
}

// ─── list_parameters ─────────────────────────────────────────────────────────

export const listParametersSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to read instead of a session (stateless path).",
    },
  },
};

interface ParameterInfo {
  name: string;
  value: Expr;
  resolved: number | null;
  unit?: string;
  min?: number;
  max?: number;
  description?: string;
}

export function listParameters(input: unknown): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;
  const { doc } = resolveDocInput(args);

  const params = doc.parameters ?? {};
  let resolved: Record<string, number> = {};
  try {
    resolved = resolveParameters(params);
  } catch {
    // A malformed/cyclic formula shouldn't sink the listing — report raw
    // values with `resolved: null` so the agent can still see and fix them.
    resolved = {};
  }

  const parameters: ParameterInfo[] = Object.entries(params).map(
    ([name, p]) => {
      const param = p as Parameter;
      const info: ParameterInfo = {
        name,
        value: param.value,
        resolved: name in resolved ? resolved[name] : null,
      };
      if (param.unit !== undefined) info.unit = param.unit;
      if (param.min !== undefined) info.min = param.min;
      if (param.max !== undefined) info.max = param.max;
      if (param.description !== undefined) info.description = param.description;
      return info;
    },
  );

  return jsonResult({ count: parameters.length, parameters });
}

// ─── set_parameters ────────────────────────────────────────────────────────────

export const setParametersSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document (required to persist).",
    },
    parameters: {
      type: "object" as const,
      description:
        "Map of parameter name → new numeric value, e.g. { \"r\": 12, \"h\": 8 }. " +
        "Every name must already be declared in the document's parameters.",
      additionalProperties: { type: "number" as const },
    },
  },
  required: ["document_id", "parameters"],
};

interface ParameterDelta {
  name: string;
  previous: Expr;
  value: number;
}

export async function setParameters(input: unknown, engine: Engine): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  if (!documentId) {
    throw new Error("`document_id` is required for set_parameters.");
  }
  const updates = args.parameters;
  if (!updates || typeof updates !== "object" || Array.isArray(updates)) {
    throw new Error(
      "`parameters` must be an object mapping parameter name → numeric value.",
    );
  }

  const doc: Document = getSession(documentId);
  doc.parameters ??= {};

  // Validate up front so a bad entry never leaves a half-applied batch.
  const entries = Object.entries(updates as Record<string, unknown>);
  const unknown: string[] = [];
  for (const [name, value] of entries) {
    if (!(name in doc.parameters)) unknown.push(name);
    else if (typeof value !== "number" || !Number.isFinite(value)) {
      throw new Error(
        `Parameter "${name}" must be set to a finite number (got ${JSON.stringify(value)}).`,
      );
    }
  }
  if (unknown.length > 0) {
    throw new Error(
      `Unknown parameter(s): ${unknown.join(", ")}. Use list_parameters to see declared names.`,
    );
  }

  const changed: ParameterDelta[] = [];
  for (const [name, value] of entries) {
    const param = doc.parameters[name] as Parameter;
    changed.push({ name, previous: param.value, value: value as number });
    param.value = value as number;
  }

  // Parameters drive constraint dimensions ("board_width - 2*margin") —
  // re-solve the document's design constraints so a parameter change
  // relayouts everything bound to it in the same call.
  let constraintSolve: Record<string, unknown> | undefined;
  if ((doc.constraints ?? []).length > 0) {
    const outcome = await solveDesignConstraints(doc);
    if (outcome.status === "ok") {
      doc.nodes = outcome.value.document.nodes;
      doc.constraints = outcome.value.document.constraints;
      constraintSolve = outcome.value.report as unknown as Record<string, unknown>;
    } else {
      constraintSolve = { unavailable: outcome.reason };
    }
  }

  const result: ToolResult = jsonResult({
    document_id: documentId,
    updated: changed.length,
    changed,
    ...(constraintSolve ? { constraint_solve: constraintSolve } : {}),
  });
  result.structuredContent = { changed };

  // Geometry changed under the new parameter values — carry an integrity
  // certificate like every other mutation.
  const integrity = computeIntegrity(doc, engine);
  if (integrity) {
    appendIntegrity(result, integrity);
    recordTriangles(documentId, integrity.triangles);
  }

  return result;
}

// ─── parameter_gradient ─────────────────────────────────────────────────────────

export const parameterGradientSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to differentiate instead of a session (stateless path).",
    },
    parameter: {
      type: "string" as const,
      description:
        "Name of the parameter to differentiate. Must be declared in the " +
        "document's parameters and bound onto some geometry field.",
    },
    density: {
      type: "number" as const,
      description:
        "Density fed to the mass integrals (mass = density · volume). Defaults to 1 (raw volume moments).",
    },
    probe_step: {
      type: "number" as const,
      description:
        "Finite step for seeding synthesis (surface matching). The reported " +
        "volume/mass/centroid derivatives are analytic, not finite differences. Defaults to 1e-4.",
    },
  },
  required: ["parameter"],
};

export function parameterGradient(input: unknown, engine: Engine): ToolResult {
  const args = (input ?? {}) as Record<string, unknown>;
  const parameter = args.parameter;
  if (typeof parameter !== "string" || parameter.length === 0) {
    throw new Error("`parameter` (the parameter name to differentiate) is required.");
  }
  const { doc } = resolveDocInput(args);

  const density =
    typeof args.density === "number" ? (args.density as number) : 1.0;
  const probeStep =
    typeof args.probe_step === "number" ? (args.probe_step as number) : 0;

  const parts: PartParameterGradient[] = engine.parameterGradient(
    doc,
    parameter,
    { density, probeStep },
  );

  return jsonResult({ parameter, density, parts });
}

export const toolDefs: ToolDef[] = [
  {
    name: "list_parameters",
    pack: null,
    description:
      "List a document's named parameters: raw expression, resolved numeric value, unit, and scrub bounds (min/max). Pair with set_parameters to drive the design and parameter_gradient to differentiate it.",
    inputSchema: listParametersSchema,
    handler: (a) => listParameters(a),
    behavior: behavior({}),
  },
  {
    name: "set_parameters",
    pack: null,
    description:
      "Batch-update named parameter values on an open session document (e.g. { \"r\": 12, \"h\": 8 }). Every name must already be declared; values must be finite numbers. Returns a `changed` diff of the deltas and re-evaluates geometry integrity. When the document has design constraints, they re-solve automatically (constraint_solve in the result) — a parameter change relayouts everything bound to it. Undoable and persisted.",
    inputSchema: setParametersSchema,
    // Batch-updates named parameters → geometry changes, so it must
    // snapshot (undo) and persist like any other document mutator.
    handler: (a, c) => setParameters(a, c.engine),
    behavior: behavior({ writesDoc: true }),
  },
  {
    name: "parameter_gradient",
    pack: null,
    description:
      "Differentiate a document's QoIs with respect to a single named parameter via the differentiable seam: per solid part, returns d(volume)/d\u03b8, d(mass)/d\u03b8, d(centroid)/d\u03b8 (exact analytic seam derivatives) and d(bbox extents)/d\u03b8 (finite difference), alongside each QoI's value. The parameter must be bound onto some geometry field. \"Solve for the geometry\" starts here.",
    inputSchema: parameterGradientSchema,
    handler: (a, c) => parameterGradient(a, c.engine),
    behavior: behavior({}),
  },
];
