/**
 * import_kicad / import_eagle — read-only import of existing PCB layouts.
 *
 * KiCad (.kicad_pcb, s-expression) is parsed by the kernel (Rust
 * `parse_kicad_pcb`, WASM `parseKicadPcb`) into the vcad `Pcb` IR, wrapped in a
 * PcbBoard session document, and registered so the returned `document_id` flows
 * straight into render_pcb / run_drc / get_pad_positions / export_gerber /
 * route_nets. Eagle (.brd, XML) is a clear "not yet" stub.
 */

import type { Pcb } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { parseKicadPcb } from "@vcad/engine";
import { readFileSync, existsSync, statSync } from "node:fs";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment } from "./remote.js";
import { registerSession } from "./session.js";

/** Cap imports at 64 MB so a remote caller can't pin memory. */
const MAX_PCB_BYTES = 64 * 1024 * 1024;

interface ImportPcbInput {
  filename?: string;
  content_base64?: string;
  name?: string;
}

function importError(text: string) {
  return {
    content: [{ type: "text" as const, text: JSON.stringify({ error: text }) }],
    isError: true as const,
  };
}

/** Read the file contents from base64 (preferred) or a sandboxed path. */
function readSource(input: ImportPcbInput, label: string): string {
  const { filename, content_base64 } = input;
  if (content_base64) {
    const buf = Buffer.from(content_base64, "base64");
    if (buf.length === 0) throw new Error("content_base64 decoded to zero bytes");
    if (buf.length > MAX_PCB_BYTES) {
      throw new Error(`${label} content exceeds the ${MAX_PCB_BYTES} byte limit`);
    }
    return buf.toString("utf8");
  }
  if (filename) {
    if (isRemoteDeployment()) {
      throw new Error(
        `This hosted server has no filesystem access — pass the ${label} contents ` +
          "as `content_base64` instead of `filename`.",
      );
    }
    const filepath = resolveWithinRoot(
      filename,
      process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd(),
    );
    if (!existsSync(filepath)) throw new Error(`${label} file not found`);
    const stat = statSync(filepath);
    if (!stat.isFile()) throw new Error(`${label} path is not a regular file`);
    if (stat.size > MAX_PCB_BYTES) {
      throw new Error(`${label} file exceeds the ${MAX_PCB_BYTES} byte limit`);
    }
    return readFileSync(filepath, "utf8");
  }
  throw new Error("Provide either `filename` or `content_base64`");
}

/** Board name from the explicit arg, else the filename stem, else a default. */
function boardName(input: ImportPcbInput): string {
  if (input.name) return input.name;
  if (input.filename) {
    return input.filename.replace(/^.*[/\\]/, "").replace(/\.kicad_pcb$/i, "");
  }
  return "Imported board";
}

export const importKicadSchema = {
  type: "object" as const,
  properties: {
    filename: {
      type: "string" as const,
      description:
        "Path to a .kicad_pcb file on the server filesystem (relative to " +
        "VCAD_MCP_EXPORT_DIR or cwd). On hosted servers pass content_base64 instead.",
    },
    content_base64: {
      type: "string" as const,
      description: "Base64-encoded .kicad_pcb contents (preferred for hosted servers).",
    },
    name: {
      type: "string" as const,
      description: "Board name for display (default: filename without extension).",
    },
  },
};

/**
 * Import a KiCad `.kicad_pcb` into a live session: parses the board outline,
 * footprints (with pads + nets), nets, design rules, and any traces/vias/zones,
 * then returns a `document_id` ready for the rest of the PCB toolchain.
 */
export async function importKicad(input: unknown) {
  const args = (input ?? {}) as ImportPcbInput;

  let content: string;
  try {
    content = readSource(args, "KiCad");
  } catch (e) {
    return importError(e instanceof Error ? e.message : String(e));
  }

  let pcb: Pcb | null;
  try {
    pcb = await parseKicadPcb(content);
  } catch (e) {
    return importError(
      `Could not parse the KiCad file (${e instanceof Error ? e.message : String(e)}). ` +
        "Re-export from KiCad 7+ and confirm it is a .kicad_pcb board file (not a project or schematic).",
    );
  }
  if (!pcb) {
    return importError(
      "KiCad parsing is unavailable (kernel WASM is missing parseKicadPcb) or the file produced no board.",
    );
  }

  // Wrap the parsed board as a PcbBoard session document (same shape
  // place_components produces), so the id works with every PCB tool.
  const doc = createDocument();
  doc.nodes["1"] = {
    id: 1,
    name: boardName(args),
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    op: { type: "PcbBoard", board: pcb } as any,
  };
  doc.roots.push({ root: 1, material: "__pcb_fr4__" });
  doc.materials["__pcb_fr4__"] = {
    name: "__pcb_fr4__",
    color: [0.05, 0.35, 0.15],
    roughness: 0.6,
    metallic: 0.0,
  };
  const documentId = registerSession(doc);

  const onlyPlacement = pcb.traces.length === 0 && pcb.vias.length === 0;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          document_id: documentId,
          summary: {
            footprints: pcb.footprints.length,
            nets: pcb.nets.length,
            outline_vertices: pcb.outline?.vertices?.length ?? 0,
            traces: pcb.traces.length,
            vias: pcb.vias.length,
            zones: pcb.zones.length,
            design_rules: {
              trace_width: pcb.rules?.defaultRules?.traceWidth,
              clearance: pcb.rules?.defaultRules?.clearance,
              via_diameter: pcb.rules?.defaultRules?.viaDiameter,
            },
          },
          ...(onlyPlacement
            ? {
                note: "Placement + netlist only — the file carried no routed copper.",
              }
            : {}),
        }),
      },
    ],
  };
}

export const importEagleSchema = {
  type: "object" as const,
  properties: {
    filename: {
      type: "string" as const,
      description: "Path to a .brd file (Eagle XML format).",
    },
    content_base64: {
      type: "string" as const,
      description: "Base64-encoded .brd contents.",
    },
  },
};

/** Eagle `.brd` (XML) import is not yet implemented — point at the KiCad path. */
export function importEagle(_input: unknown) {
  return importError(
    "Eagle (.brd) import is not yet supported. Export your board from Eagle as " +
      "KiCad (File > Export > KiCad .kicad_pcb) and use import_kicad instead.",
  );
}
