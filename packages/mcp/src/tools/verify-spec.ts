/**
 * verify_spec — grade an open session document against a caller-supplied
 * spec and return a fail-closed DesignReceipt. "TDD for CAD": declare the
 * spec first (bbox, volume range, watertight, part count, center of mass),
 * then iterate the geometry until every claim rolls up to pass.
 *
 * The measurement comes from the kernel tessellation via `computeIntegrity`
 * (the same trust layer under every mutation). The receipt reuses the unified
 * schema (`receipt-unified.ts`) and its fail-closed rollup: an empty spec, a
 * missing measurement, or a claim the kernel can't evaluate is `unverifiable`,
 * never a silent pass. Unlike verify_part, this touches no mecheval grading.
 */

import { createHash } from "node:crypto";
import type { Engine } from "@vcad/engine";
import { computeIntegrity } from "./integrity.js";
import { getSession } from "./session.js";
import {
  summarize,
  unifiedFromSpec,
  type DesignSpec,
  type SpecMeasurement,
} from "../receipt-unified.js";

const pointSpecSchema = {
  type: "object" as const,
  properties: {
    x: { type: "number" as const },
    y: { type: "number" as const },
    z: { type: "number" as const },
    tol: {
      type: "number" as const,
      description: "± tolerance in mm applied per declared axis (default 0.01).",
    },
  },
};

export const verifySpecSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document — the document to grade.",
    },
    spec: {
      type: "object" as const,
      description:
        "The spec to verify. Every field is optional; each declared field " +
        "produces one or more claims reporting measured-vs-expected. A spec " +
        "that declares nothing is unverifiable (no evidence), never a pass.",
      properties: {
        bbox_min: {
          ...pointSpecSchema,
          description: "Bounding-box minimum corner (any subset of axes) ± tol.",
        },
        bbox_max: {
          ...pointSpecSchema,
          description: "Bounding-box maximum corner (any subset of axes) ± tol.",
        },
        volume: {
          type: "object" as const,
          description: "Enclosed volume must fall within [min, max] mm³ (either bound optional).",
          properties: {
            min: { type: "number" as const, description: "Minimum volume (mm³)." },
            max: { type: "number" as const, description: "Maximum volume (mm³)." },
          },
        },
        watertight: {
          type: "boolean" as const,
          description: "Whether the solid must be a closed, watertight manifold.",
        },
        part_count: {
          type: "integer" as const,
          description: "Exact number of parts the document must contain.",
        },
        center_of_mass: {
          ...pointSpecSchema,
          description: "Center of mass (any subset of axes) ± tol.",
        },
      },
    },
  },
  required: ["document_id", "spec"],
};

/**
 * Grade the session document against `spec` and return the unified receipt
 * plus its summary. A failing or unverifiable verdict is a valid result (the
 * whole point of iterate-to-green), not a tool error.
 */
export function verifySpec(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const spec = (args.spec ?? {}) as DesignSpec;
  const doc = getSession(documentId);

  // computeIntegrity returns null when the kernel can't evaluate the document
  // at all — a null measurement makes every declared claim unverifiable.
  const report = computeIntegrity(doc, engine);
  const measurement: SpecMeasurement | null = report
    ? {
        volume_mm3: report.volume_mm3,
        bounding_box: report.bounding_box,
        center_of_mass: report.center_of_mass,
        watertight: report.watertight,
        parts: report.parts,
      }
    : null;

  // Fingerprint the design snapshot the claims were checked against — a
  // receipt without it can't prove WHICH document it certifies.
  const fingerprint = createHash("sha256")
    .update(JSON.stringify(doc))
    .digest("hex");

  const receipt = unifiedFromSpec(spec, measurement, documentId, fingerprint);
  const summary = summarize(receipt);

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ receipt, summary }, null, 2),
      },
    ],
  };
}
