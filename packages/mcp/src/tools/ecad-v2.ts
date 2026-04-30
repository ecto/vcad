/**
 * ECAD v2 wrappers — thin handle/envelope adapters over the v1 ECAD
 * tools. The underlying v1 functions still drive schematic + PCB
 * mutations; v2 just plumbs the doc through the handle store and
 * returns the universal envelope.
 *
 * Surface:
 *   schematic(create or edit)
 *   layout    (place components on a PCB)
 *   route     (route copper traces)
 *   check     (DRC + ERC, unified)
 *   gerber    (zip resource bundle)
 */

import type { Document } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import {
  createSchematic,
  placeComponents,
  routeNets,
  runDrc,
  runErc,
  exportGerber,
  calcImpedance,
} from "./ecad.js";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { commitDoc, resolveRef } from "../handles.js";
import type { DocHandle, DocRef } from "../types.js";

export const schematicSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle (omit for fresh)." },
    title: { type: "string" as const },
    components: { type: "array" as const },
    wires: { type: "array" as const },
    labels: { type: "array" as const },
  },
};

export const layoutSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle (must contain a schematic)." },
    outline: { description: "Polygon points or { width, height } for a rectangular board." },
    stackup: { description: "Layer stackup definition." },
    placements: { type: "array" as const },
  },
  required: ["doc"],
};

export const routeSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle." },
    mode: { type: "string" as const, enum: ["auto", "constrained"] },
    constraints: { type: "array" as const },
  },
  required: ["doc"],
};

export const checkSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle." },
    rules: { type: "object" as const },
  },
  required: ["doc"],
};

export const gerberSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle." },
    format: { type: "string" as const, enum: ["rs274x", "ipc2581"] },
  },
  required: ["doc"],
};

export const calcImpedanceSchemaV2 = {
  type: "object" as const,
  properties: {
    config: { type: "string" as const, enum: ["microstrip", "stripline", "differential"] },
    width: { type: "number" as const },
    height: { type: "number" as const },
    thickness: { type: "number" as const },
    er: { type: "number" as const },
    spacing: { type: "number" as const, description: "differential pair spacing" },
  },
  required: ["config", "width", "height", "thickness", "er"],
};

interface EcadV2Common {
  doc?: DocRef;
}

function unwrapV1(result: { content: Array<{ type: string; text: string }> }): unknown {
  const text = result.content.find((c) => c.type === "text") as { text: string } | undefined;
  if (!text) return null;
  try {
    return JSON.parse(text.text);
  } catch {
    return text.text;
  }
}

function maybeMergeDoc(payload: unknown, source: Document): Document {
  // v1 ecad helpers return `document: ...` in their JSON. Prefer that.
  if (payload && typeof payload === "object" && "document" in payload) {
    return (payload as { document: Document }).document;
  }
  return source;
}

export function schematicV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as EcadV2Common & {
    title?: string;
    components?: unknown[];
    wires?: unknown[];
    labels?: unknown[];
  };

  const baseHandle = typeof args.doc === "string" ? args.doc : undefined;
  const { doc: source } = args.doc
    ? resolveRef(args.doc)
    : { doc: undefined as Document | undefined };

  // Delegate to v1 createSchematic to mint a doc with the schematic on it.
  const v1 = createSchematic({
    title: args.title,
    components: args.components,
    wires: args.wires,
    labels: args.labels,
  } as Record<string, unknown>);
  const payload = unwrapV1(v1) as { document?: Document; components?: number; wires?: number; labels?: number };

  // If editing, merge the new schematic onto the existing doc.
  let doc: Document;
  if (source && payload?.document) {
    doc = JSON.parse(JSON.stringify(source));
    doc.schematic = payload.document.schematic;
  } else if (payload?.document) {
    doc = payload.document;
  } else {
    return fail("schematic_failed", "v1 createSchematic returned no document.");
  }

  const handle = commitDoc(doc, baseHandle as DocHandle | undefined);
  return ok({
    result: {
      components: payload.components,
      wires: payload.wires,
      labels: payload.labels,
    },
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}

export function layoutV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as EcadV2Common & Record<string, unknown>;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  const baseHandle = typeof args.doc === "string" ? args.doc : undefined;
  const { doc: source } = resolveRef(args.doc);
  // The v1 placeComponents signature accepts loose args — pass `document` through.
  const v1 = placeComponents({ ...args, document: source } as Record<string, unknown>);
  const payload = unwrapV1(v1) as { document?: Document } & Record<string, unknown>;
  const doc = maybeMergeDoc(payload, source);
  const handle = commitDoc(doc, baseHandle as DocHandle | undefined);
  return ok({
    result: stripDocFromPayload(payload),
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}

export function routeV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as EcadV2Common & Record<string, unknown>;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  const baseHandle = typeof args.doc === "string" ? args.doc : undefined;
  const { doc: source } = resolveRef(args.doc);
  const v1 = routeNets({ ...args, document: source } as Record<string, unknown>);
  const payload = unwrapV1(v1) as { document?: Document } & Record<string, unknown>;
  const doc = maybeMergeDoc(payload, source);
  const handle = commitDoc(doc, baseHandle as DocHandle | undefined);
  return ok({
    result: stripDocFromPayload(payload),
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}

export function checkV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as EcadV2Common & Record<string, unknown>;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  const { doc: source, handle } = resolveRef(args.doc);

  const drc = unwrapV1(runDrc({ document: source, ...(args.rules as object ?? {}) } as Record<string, unknown>)) as
    | { errors?: unknown[]; warnings?: unknown[]; info?: unknown[] }
    | null;
  const erc = unwrapV1(runErc({ document: source } as Record<string, unknown>)) as
    | { errors?: unknown[]; warnings?: unknown[]; info?: unknown[] }
    | null;

  const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
  const merge = (kind: "errors" | "warnings" | "info") => [
    ...arr(drc?.[kind]),
    ...arr(erc?.[kind]),
  ];

  return ok({
    result: {
      errors: merge("errors"),
      warnings: merge("warnings"),
      info: merge("info"),
      drc_summary: drc,
      erc_summary: erc,
    },
    handle,
    doc: source,
    engine,
    startedAt,
    skipPreview: true,
  });
}

export function gerberV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as EcadV2Common & { format?: string };
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");
  const { doc: source, handle } = resolveRef(args.doc);
  const v1 = exportGerber({ document: source } as Record<string, unknown>);
  const payload = unwrapV1(v1);
  return ok({
    result: { format: args.format ?? "rs274x", ...(payload as Record<string, unknown> ?? {}) },
    handle,
    doc: source,
    engine,
    startedAt,
    skipPreview: true,
  });
}

export function calcImpedanceV2(input: unknown): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as Record<string, unknown>;
  const v1 = calcImpedance(args);
  const payload = unwrapV1(v1);
  return ok({
    result: payload,
    handle: "vcad:doc:00000000-0000-0000-0000-000000000000",
    startedAt,
    skipPreview: true,
  });
}

function stripDocFromPayload(payload: unknown): Record<string, unknown> {
  if (!payload || typeof payload !== "object") return {};
  const copy = { ...(payload as Record<string, unknown>) };
  delete copy.document;
  return copy;
}
