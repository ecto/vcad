/**
 * import_kicad / import_altium / import_altium_library / import_eagle —
 * read-only import of existing PCB layouts.
 *
 * KiCad (.kicad_pcb, s-expression) is parsed by the kernel (Rust
 * `parse_kicad_pcb`, WASM `parseKicadPcb`) into the vcad `Pcb` IR, wrapped in a
 * PcbBoard session document, and registered so the returned `document_id` flows
 * straight into render_pcb / run_drc / get_pad_positions / export_gerber /
 * route_nets. The round-trip companion is export_kicad (native, editable
 * .kicad_pcb / .kicad_sch out).
 *
 * Altium arrives the same way through `parseAltiumAsciiPcb` /
 * `parseAltiumPcbDoc`: ASCII-exported and native binary `.PcbDoc` both land on
 * the same `Pcb` IR, with the binary path failing closed rather than importing
 * a partially-decoded board. `.PcbLib` libraries come back as footprint
 * definitions rather than a session document, since a library is not a board.
 *
 * Eagle `.brd` (XML, Eagle 6+) goes through `parseEagleBrd` onto the same IR.
 */

import type { Pcb } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import {
  parseKicadPcb,
  parseAltiumAsciiPcb,
  parseAltiumPcbDoc,
  parseAltiumPcbLib,
  parseEagleBrd,
} from "@vcad/engine";
import { readFileSync, existsSync, statSync } from "node:fs";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment } from "./remote.js";
import { registerSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";
import type { ToolResult } from "./tool-result.js";

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

/** Read the raw bytes from base64 (preferred) or a sandboxed path. */
function readSourceBytes(input: ImportPcbInput, label: string): Buffer {
  const { filename, content_base64 } = input;
  if (content_base64) {
    const buf = Buffer.from(content_base64, "base64");
    if (buf.length === 0) throw new Error("content_base64 decoded to zero bytes");
    if (buf.length > MAX_PCB_BYTES) {
      throw new Error(`${label} content exceeds the ${MAX_PCB_BYTES} byte limit`);
    }
    return buf;
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
    return readFileSync(filepath);
  }
  throw new Error("Provide either `filename` or `content_base64`");
}

/**
 * Wrap a parsed board as a PcbBoard session document (the same shape
 * place_components produces) so the returned id works with every PCB tool.
 */
function registerBoard(pcb: Pcb, name: string): string {
  const doc = createDocument();
  doc.nodes["1"] = {
    id: 1,
    name,
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
  return registerSession(doc);
}

/** The summary block every board import returns. */
function boardSummary(pcb: Pcb) {
  return {
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
  };
}

/** Board name from the explicit arg, else the filename stem, else a default. */
function boardName(input: ImportPcbInput): string {
  if (input.name) return input.name;
  if (input.filename) {
    return input.filename
      .replace(/^.*[/\\]/, "")
      .replace(/\.(kicad_pcb|pcbdoc|pcblib|brd)$/i, "");
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

  const documentId = registerBoard(pcb, boardName(args));
  const onlyPlacement = pcb.traces.length === 0 && pcb.vias.length === 0;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          document_id: documentId,
          summary: boardSummary(pcb),
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

export const importAltiumSchema = {
  type: "object" as const,
  properties: {
    filename: {
      type: "string" as const,
      description:
        "Path to a .PcbDoc file on the server filesystem (relative to " +
        "VCAD_MCP_EXPORT_DIR or cwd). Native binary and ASCII exports are both " +
        "accepted. On hosted servers pass content_base64 instead.",
    },
    content_base64: {
      type: "string" as const,
      description: "Base64-encoded .PcbDoc contents (preferred for hosted servers).",
    },
    name: {
      type: "string" as const,
      description: "Board name for display (default: filename without extension).",
    },
  },
};

/** An OLE compound file — the container Altium's native .PcbDoc/.PcbLib use. */
function isCompoundFile(bytes: Buffer): boolean {
  return (
    bytes.length >= 8 &&
    bytes
      .subarray(0, 8)
      .equals(Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]))
  );
}

/**
 * Import an Altium `.PcbDoc` into a live session. The flavour is detected from
 * the file's own bytes (OLE signature = native binary, otherwise ASCII), so
 * callers never have to declare which export they have.
 *
 * The binary path fails closed: rather than importing a board whose copper may
 * be silently misplaced by an unrecognised record layout, it errors and names
 * the ASCII export as the fallback.
 */
export async function importAltium(input: unknown) {
  const args = (input ?? {}) as ImportPcbInput;

  let bytes: Buffer;
  try {
    bytes = readSourceBytes(args, "Altium");
  } catch (e) {
    return importError(e instanceof Error ? e.message : String(e));
  }

  const binary = isCompoundFile(bytes);
  let pcb: Pcb | null;
  try {
    pcb = binary
      ? await parseAltiumPcbDoc(new Uint8Array(bytes))
      : await parseAltiumAsciiPcb(bytes.toString("utf8"));
  } catch (e) {
    // The kernel's message is the actionable part (which stream failed, and
    // that PCB ASCII is the way out) — surface it rather than replacing it.
    return importError(
      `Could not parse the Altium ${binary ? "binary" : "ASCII"} .PcbDoc: ` +
        (e instanceof Error ? e.message : String(e)),
    );
  }
  if (!pcb) {
    return importError(
      "Altium parsing is unavailable (kernel WASM is missing parseAltiumPcbDoc).",
    );
  }

  const documentId = registerBoard(pcb, boardName(args));
  const onlyPlacement = pcb.traces.length === 0 && pcb.vias.length === 0;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          document_id: documentId,
          source_format: binary ? "PcbDoc (binary)" : "PcbDoc (ASCII)",
          summary: boardSummary(pcb),
          ...(onlyPlacement
            ? { note: "Placement + netlist only — the file carried no routed copper." }
            : {}),
          ...(binary
            ? {
                note_binary:
                  "Binary .PcbDoc record layouts are reconstructed, not specified by " +
                  "Altium, and are validated against real open-hardware projects. " +
                  "Geometry that decoded implausibly fails the import outright rather " +
                  "than importing wrong, but spot-check with render_pcb before " +
                  "fabricating. Copper pours and text are not imported.",
              }
            : {}),
        }),
      },
    ],
  };
}

export const importAltiumLibrarySchema = {
  type: "object" as const,
  properties: {
    filename: {
      type: "string" as const,
      description: "Path to a .PcbLib footprint library (binary or ASCII).",
    },
    content_base64: {
      type: "string" as const,
      description: "Base64-encoded .PcbLib contents.",
    },
  },
};

/**
 * Read an Altium `.PcbLib` footprint library. A library is not a board, so
 * this returns footprint definitions (pads, shapes, drills) rather than
 * minting a session document.
 */
export async function importAltiumLibrary(input: unknown) {
  const args = (input ?? {}) as ImportPcbInput;

  let bytes: Buffer;
  try {
    bytes = readSourceBytes(args, "Altium library");
  } catch (e) {
    return importError(e instanceof Error ? e.message : String(e));
  }

  let lib: Awaited<ReturnType<typeof parseAltiumPcbLib>>;
  try {
    lib = await parseAltiumPcbLib(new Uint8Array(bytes));
  } catch (e) {
    return importError(
      `Could not parse the Altium .PcbLib: ${e instanceof Error ? e.message : String(e)}`,
    );
  }
  if (!lib) {
    return importError(
      "Altium parsing is unavailable (kernel WASM is missing parseAltiumPcbLib).",
    );
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          footprint_count: lib.footprints.length,
          footprints: lib.footprints.map((f) => ({
            name: f.name,
            pad_count: f.pads.length,
            pads: f.pads.map((p) => ({
              number: p.number,
              type: p.pad_type,
              position: p.position,
              rotation: p.rotation,
              drill: p.drill?.diameter ?? null,
            })),
          })),
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
      description: "Path to a .brd file (Eagle XML, Eagle 6+).",
    },
    content_base64: {
      type: "string" as const,
      description: "Base64-encoded .brd contents (preferred for hosted servers).",
    },
    name: {
      type: "string" as const,
      description: "Board name for display (default: filename without extension).",
    },
  },
};

/**
 * Import an Eagle `.brd` (XML, Eagle 6+) into a live session: board outline
 * from the layer-20 graphics, packages and their placements as footprints with
 * pads, and the signals as nets plus any hand-routed traces and vias.
 *
 * Eagle's pre-6 binary `.brd` is not XML and is rejected with that explanation
 * rather than being half-parsed.
 */
export async function importEagle(input: unknown) {
  const args = (input ?? {}) as ImportPcbInput;

  let content: string;
  try {
    content = readSource(args, "Eagle");
  } catch (e) {
    return importError(e instanceof Error ? e.message : String(e));
  }
  if (!content.includes("<eagle") && !content.includes("<board")) {
    return importError(
      "This does not look like an Eagle XML .brd file. Eagle 5 and earlier wrote " +
        "a binary .brd — open it in Eagle 6+ (which converts it to XML) and save, " +
        "then import that.",
    );
  }

  let pcb: Pcb | null;
  try {
    pcb = await parseEagleBrd(content);
  } catch (e) {
    return importError(
      `Could not parse the Eagle .brd: ${e instanceof Error ? e.message : String(e)}`,
    );
  }
  if (!pcb) {
    return importError(
      "Eagle parsing is unavailable (kernel WASM is missing parseEagleBrd).",
    );
  }

  const documentId = registerBoard(pcb, boardName(args));
  const onlyPlacement = pcb.traces.length === 0 && pcb.vias.length === 0;
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify({
          success: true,
          document_id: documentId,
          summary: boardSummary(pcb),
          ...(onlyPlacement
            ? { note: "Placement + netlist only — the file carried no routed copper." }
            : {}),
        }),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "import_kicad",
    pack: "ecad",
    description:
      "Import an existing KiCad .kicad_pcb board into a live session — board " +
      "outline, footprints with pads + nets, design rules, and any routed " +
      "traces/vias/zones. Returns a document_id ready for render_pcb, " +
      "run_drc, get_pad_positions, route_nets, and export_gerber. Pass " +
      "content_base64 on hosted servers.",
    inputSchema: importKicadSchema,
    handler: async (a) => (await importKicad(a)) as unknown as ToolResult,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "import_altium",
    pack: "ecad",
    description:
      "Import an Altium .PcbDoc board into a live session — board outline, " +
      "footprints with pads + nets, design rules, and any routed traces/vias. " +
      "Accepts both the native binary file and an ASCII export (detected " +
      "automatically). Returns a document_id ready for render_pcb, run_drc, " +
      "get_pad_positions, route_nets, and export_gerber. The binary path fails " +
      "closed on record layouts it cannot decode rather than importing a " +
      "partially-correct board. Pass content_base64 on hosted servers.",
    inputSchema: importAltiumSchema,
    handler: async (a) => (await importAltium(a)) as unknown as ToolResult,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
  {
    name: "import_altium_library",
    pack: "ecad",
    description:
      "Read an Altium .PcbLib footprint library (binary or ASCII) and return " +
      "its footprint patterns with pads, shapes, and drills. A library is not " +
      "a board, so no document_id is minted.",
    inputSchema: importAltiumLibrarySchema,
    handler: async (a) => (await importAltiumLibrary(a)) as unknown as ToolResult,
    behavior: behavior({}),
  },
  {
    name: "import_eagle",
    pack: "ecad",
    description:
      "Import an Eagle .brd board (XML, Eagle 6+) into a live session — board " +
      "outline, packages placed as footprints with pads, signals as nets, and " +
      "any hand-routed traces/vias. Returns a document_id ready for render_pcb, " +
      "run_drc, get_pad_positions, route_nets, and export_gerber. Pass " +
      "content_base64 on hosted servers.",
    inputSchema: importEagleSchema,
    handler: async (a) => (await importEagle(a)) as unknown as ToolResult,
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
];
