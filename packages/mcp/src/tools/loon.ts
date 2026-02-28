/**
 * create_cad_loon tool — evaluate loon source to produce a CAD document.
 */

import type { Engine } from "@vcad/engine";
import { toVCode } from "@vcad/ir";

/** JSON Schema for create_cad_loon input. */
export const createCadLoonSchema = {
  type: "object" as const,
  properties: {
    source: {
      type: "string" as const,
      description: "Loon source code defining CAD geometry",
    },
    format: {
      type: "string" as const,
      enum: ["vcode", "json"],
      description: "Output format (default: compact)",
    },
  },
  required: ["source"],
};

interface CreateLoonInput {
  source: string;
  format?: "vcode" | "json";
}

/** Evaluate loon source and return a CAD document. */
export function createCadLoon(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const { source, format = "vcode" } = input as CreateLoonInput;

  const doc = engine.evalVcadSource(source);
  if (!doc) {
    return {
      content: [{ type: "text", text: "Error: Loon evaluation not supported by this engine build" }],
    };
  }

  const text = format === "json" ? JSON.stringify(doc, null, 2) : toVCode(doc);

  return {
    content: [{ type: "text", text }],
  };
}
