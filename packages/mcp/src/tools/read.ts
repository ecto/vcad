/**
 * read tool — hydrate a doc handle to its full IR JSON.
 *
 * This is the escape hatch for IR-fluent agents. The universal envelope
 * already returns stats + preview on every build/edit, so most agents
 * never need raw IR. When they do (e.g. inspecting topology, handing
 * off to an external tool), this is the canonical accessor.
 */

import { toJson, toVCode } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef } from "../types.js";

export const readSchema = {
  type: "object" as const,
  properties: {
    doc: {
      description: "Handle (`vcad:doc:<uuid>[@<version>]`) or inline IR document.",
    },
    format: {
      type: "string" as const,
      enum: ["json", "vcode"],
      description: "Serialization format. Defaults to `json`.",
    },
  },
  required: ["doc"],
};

interface ReadInput {
  doc: DocRef;
  format?: "json" | "vcode";
}

export function readTool(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as ReadInput;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");

  const { doc, handle } = resolveRef(args.doc);
  const format = args.format ?? "json";
  const ir = format === "vcode" ? toVCode(doc) : toJson(doc);

  return ok({
    result: { format, ir },
    handle,
    doc,
    engine,
    startedAt,
    skipPreview: true,
  });
}
