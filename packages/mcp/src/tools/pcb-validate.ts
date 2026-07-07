/**
 * Shared PCB document-validity gate for MCP tools.
 *
 * Every tool that serializes a Pcb to JSON for the kernel WASM (render_pcb,
 * render_stackup, export_gerber, build_receipt, run_drc) MUST call
 * `validatePcb()` first. A malformed board (e.g. a dotted layer name like
 * "F.Cu" that serde rejects) will fail render/export but silently pass DRC
 * (which swallows the serde error and returns 0 violations). This one gate
 * ensures all tools fail-closed on the same conditions.
 */

import type { Pcb } from "@vcad/ir";
import { PCB_LAYERS } from "./pcb-layers.js";

const VALID_LAYERS: ReadonlySet<string> = new Set<string>(PCB_LAYERS);

export { VALID_LAYERS };

export interface PcbDiagnostic {
  subsystem: string;
  field: string;
  value: unknown;
  accepted?: string[];
  message: string;
}

export interface PcbValidationResult {
  valid: boolean;
  diagnostics: PcbDiagnostic[];
  documentSafe: boolean;
}

export function validatePcb(pcb: Pcb): PcbValidationResult {
  const diagnostics: PcbDiagnostic[] = [];

  if (!pcb.stackup || !Array.isArray(pcb.stackup.layers)) {
    diagnostics.push({
      subsystem: "serde",
      field: "pcb.stackup.layers",
      value: pcb.stackup,
      message: "stackup.layers is missing or not an array",
    });
    return { valid: false, diagnostics, documentSafe: true };
  }

  const accepted = [...VALID_LAYERS];

  for (let i = 0; i < pcb.stackup.layers.length; i++) {
    const layer = pcb.stackup.layers[i];
    if (!layer || typeof layer.layer !== "string") {
      diagnostics.push({
        subsystem: "layer_parse",
        field: `pcb.stackup.layers[${i}].layer`,
        value: layer?.layer,
        accepted,
        message: `stackup layer ${i} has no layer name`,
      });
      continue;
    }
    if (!VALID_LAYERS.has(layer.layer)) {
      diagnostics.push({
        subsystem: "layer_parse",
        field: `pcb.stackup.layers[${i}].layer`,
        value: layer.layer,
        accepted,
        message: `"${layer.layer}" is not a valid PcbLayer — serde will reject this when the board is serialized for render/export/DRC`,
      });
    }
  }

  for (const trace of pcb.traces ?? []) {
    if (typeof trace.layer === "string" && !VALID_LAYERS.has(trace.layer)) {
      diagnostics.push({
        subsystem: "layer_parse",
        field: "pcb.traces[].layer",
        value: trace.layer,
        accepted,
        message: `trace on invalid layer "${trace.layer}"`,
      });
      break;
    }
  }

  for (const via of pcb.vias ?? []) {
    if (typeof (via as { startLayer?: string }).startLayer === "string" &&
        !VALID_LAYERS.has((via as { startLayer: string }).startLayer)) {
      diagnostics.push({
        subsystem: "layer_parse",
        field: "pcb.vias[].startLayer",
        value: (via as { startLayer: string }).startLayer,
        accepted,
        message: `via with invalid startLayer "${(via as { startLayer: string }).startLayer}"`,
      });
      break;
    }
  }

  for (const zone of pcb.zones ?? []) {
    if (typeof zone.layer === "string" && !VALID_LAYERS.has(zone.layer)) {
      diagnostics.push({
        subsystem: "layer_parse",
        field: "pcb.zones[].layer",
        value: zone.layer,
        accepted,
        message: `zone on invalid layer "${zone.layer}"`,
      });
      break;
    }
  }

  return {
    valid: diagnostics.length === 0,
    diagnostics,
    documentSafe: true,
  };
}

export function pcbValidationError(
  toolName: string,
  result: PcbValidationResult,
  documentId?: string,
) {
  const first = result.diagnostics[0]!;
  const errorBody: Record<string, unknown> = {
    error: first.message,
    subsystem: first.subsystem,
    field: first.field,
    value: first.value,
    document_safe: result.documentSafe,
    tool: toolName,
  };
  if (first.accepted) errorBody.accepted = first.accepted;
  if (documentId) errorBody.document_id = documentId;
  if (result.diagnostics.length > 1) {
    errorBody.other_issues = result.diagnostics.slice(1).map(d => ({
      field: d.field,
      value: d.value,
      message: d.message,
    }));
  }

  return {
    content: [{ type: "text" as const, text: JSON.stringify(errorBody) }],
    isError: true as const,
  };
}
