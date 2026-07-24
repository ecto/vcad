/**
 * simulate_strike — the receipt you can hear, before the metal exists.
 *
 * Models a mallet strike on a flat free-free bar (a glockenspiel /
 * vibraphone bar) and returns what the aluminum will do: the modal
 * frequency table, a synthesized WAV of the strike, and a verdict from
 * FFT peak extraction against the target pitch, in cents.
 *
 * All numerics live in the kernel (`vcad_kernel_acoustics::strike`, reached
 * through the WASM binding `simulateStrikeKernel`): closed-form
 * Euler–Bernoulli modes, the hole-aware 1-D Hermite-beam FEM, strike-excited
 * modal synthesis, the WAV encoder, and the FFT round trip for the verdict.
 * This file only marshals: bar geometry out of the session document, material
 * properties out of the kernel registry, kernel results into the tool payload.
 *
 * The returned `modes` table is also the data contract for client-side
 * Web Audio synthesis in the viewer widget (tap a bar, hear the note) —
 * a few hundred bytes instead of an audio blob.
 */

import type { Engine } from "@vcad/engine";
import { writeFileSync } from "node:fs";
import { getSession } from "./session.js";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment, maxInlineExportBytes } from "./remote.js";

export interface BarSpec {
  length_mm: number;
  width_mm: number;
  thickness_mm: number;
  /** Hole centers along the bar axis (mm from one end). */
  holes_mm: number[];
  hole_dia_mm: number;
  /** Young's modulus (GPa) and density (kg/m³) — from the kernel registry. */
  modulus_gpa: number;
  density_kg_m3: number;
}

export interface Mode {
  n: number;
  hz: number;
  /** Linear amplitude, ≤ 1 (mode shape at strike × mallet spectrum). */
  gain: number;
  q: number;
  /** Time to −60 dB (s). */
  t60_s: number;
}

export interface SpectralPeak {
  hz: number;
  db: number;
}

/** Wire mirror of the kernel's `StrikeResult` (WAV as base64). */
interface KernelStrikeResult {
  closed_form_hz: number[];
  fem_hz: number[];
  modes: Mode[];
  spectrum_peaks: SpectralPeak[];
  verdict: {
    expected_hz: number;
    measured_hz: number;
    cents_error: number;
    tolerance_cents: number;
    pass: boolean;
  } | null;
  wav_base64: string | null;
}

export const cents = (a: number, b: number) => 1200 * Math.log2(a / b);

// ─── Marshalling: material registry and session document ─────────────────

/** Aliases into the kernel registry, mirroring materials.rs `lookup`. */
const MATERIAL_ALIASES: Record<string, string> = {
  "al-6061": "al-hard", "6061": "al-hard", "6061-t6": "al-hard",
  aluminum: "al-soft", aluminium: "al-soft", al: "al-soft",
  "al-1100": "al-soft", "al-3003": "al-soft", "1100": "al-soft", "3003": "al-soft",
  steel: "steel-mild", "mild-steel": "steel-mild", a36: "steel-mild",
  crs: "steel-mild", "1018": "steel-mild",
  stainless: "ss-304", ss: "ss-304", "304": "ss-304", ss304: "ss-304",
  cu: "copper",
};

function lookupMaterial(engine: Engine, name: string) {
  const key = name.trim().toLowerCase().replace(/[_ ]/g, "-");
  const registry = engine.getSheetMetalMaterials() as Array<{
    name: string;
    display_name: string;
    modulus_gpa: number;
    density_kg_m3: number;
  }>;
  const resolved = MATERIAL_ALIASES[key] ?? key;
  const hit = registry.find((m) => m.name.toLowerCase() === resolved);
  if (!hit) {
    throw new Error(
      `material ${JSON.stringify(name)} not in the kernel registry — known: ${registry.map((m) => m.name).join(", ")}`,
    );
  }
  return hit;
}

/** Pull bar geometry out of a flat single-panel sheet-metal session doc. */
function barFromDocument(engine: Engine, documentId: string): {
  bar: Omit<BarSpec, "modulus_gpa" | "density_kg_m3">;
  material: string;
} {
  const doc = getSession(documentId);
  const nodes = Object.values(doc.nodes ?? {}) as Array<{
    op: Record<string, unknown> & { type: string };
  }>;
  if (nodes.some((n) => ["SheetMetalEdgeFlange", "SheetMetalHem", "SheetMetalJog"].includes(n.op.type))) {
    throw new Error(
      "strike simulation models flat free-free bars — this document has bends. Simulate the bars, not the stand.",
    );
  }
  const base = nodes.find((n) =>
    ["SheetMetalBaseFlangePolygon", "SheetMetalBaseFlangeRect"].includes(n.op.type),
  );
  if (!base) throw new Error("document has no sheet-metal base flange");
  const op = base.op as {
    type: string;
    outline?: { x: number; y: number }[];
    holes?: { x: number; y: number }[][];
    width?: number;
    depth?: number;
    thickness: number;
    material?: string;
  };
  let lengthMm: number;
  let widthMm: number;
  let axis: "x" | "y";
  if (op.type === "SheetMetalBaseFlangeRect") {
    lengthMm = Math.max(op.width ?? 0, op.depth ?? 0);
    widthMm = Math.min(op.width ?? 0, op.depth ?? 0);
    axis = (op.width ?? 0) >= (op.depth ?? 0) ? "x" : "y";
  } else {
    const xs = (op.outline ?? []).map((p) => p.x);
    const ys = (op.outline ?? []).map((p) => p.y);
    const ex = Math.max(...xs) - Math.min(...xs);
    const ey = Math.max(...ys) - Math.min(...ys);
    axis = ex >= ey ? "x" : "y";
    lengthMm = Math.max(ex, ey);
    widthMm = Math.min(ex, ey);
  }
  const holeLoops = op.holes ?? [];
  const holesMm: number[] = [];
  let holeDia = 0;
  for (const loop of holeLoops) {
    const cs = loop.map((p) => (axis === "x" ? p.x : p.y));
    holesMm.push((Math.min(...cs) + Math.max(...cs)) / 2);
    holeDia = Math.max(holeDia, Math.max(...cs) - Math.min(...cs));
  }
  return {
    bar: {
      length_mm: lengthMm,
      width_mm: widthMm,
      thickness_mm: op.thickness,
      holes_mm: holesMm,
      hole_dia_mm: holeDia,
    },
    material: op.material ?? "al-soft",
  };
}

// ─── The MCP tool ─────────────────────────────────────────────────────────

export const simulateStrikeSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "Session id of a FLAT sheet-metal part (from sheet_metal_create) — the bar's length, width, thickness, material, and hole positions are read from it. Mutually exclusive with `bar`.",
    },
    bar: {
      type: "object" as const,
      description:
        "Explicit bar spec instead of a document: {length_mm, width_mm, thickness_mm, material?, holes_mm?, hole_dia_mm?}.",
    },
    note: {
      type: "string" as const,
      description:
        'Target pitch to verify against, e.g. "C6", "F#4". Alternative to expect_hz.',
    },
    expect_hz: {
      type: "number" as const,
      description: "Target fundamental (Hz) to verify against.",
    },
    tolerance_cents: {
      type: "number" as const,
      description: "Pass/fail tolerance for the verdict (cents). Default 10.",
    },
    strike_position: {
      type: "number" as const,
      description:
        "Strike point as a fraction of bar length, 0..1. Default 0.5 (center — antinode of mode 1, node of mode 2, like a real player).",
    },
    mallet_contact_ms: {
      type: "number" as const,
      description:
        "Mallet contact time (ms). Hard glockenspiel mallet ≈ 0.5 (default); soft ≈ 2 — longer contact low-passes the upper partials.",
    },
    duration_s: {
      type: "number" as const,
      description: "Length of the synthesized strike (s). Default 2.5.",
    },
    wav_filename: {
      type: "string" as const,
      description:
        "Write the synthesized strike as a 16-bit/44.1kHz WAV with this name (local servers write to the export dir; hosted returns base64). Omit to skip audio and return the modal/verdict data only.",
    },
  },
};

export function simulateStrike(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  let barDims: Omit<BarSpec, "modulus_gpa" | "density_kg_m3">;
  let materialName: string;
  if (typeof a.document_id === "string" && a.document_id.length > 0) {
    const r = barFromDocument(engine, a.document_id);
    barDims = r.bar;
    materialName = r.material;
  } else if (a.bar && typeof a.bar === "object") {
    const b = a.bar as Record<string, unknown>;
    for (const f of ["length_mm", "width_mm", "thickness_mm"]) {
      if (typeof b[f] !== "number") throw new Error(`bar.${f} (number) is required`);
    }
    barDims = {
      length_mm: Number(b.length_mm),
      width_mm: Number(b.width_mm),
      thickness_mm: Number(b.thickness_mm),
      holes_mm: Array.isArray(b.holes_mm) ? (b.holes_mm as number[]).map(Number) : [],
      hole_dia_mm: Number(b.hole_dia_mm ?? 0),
    };
    materialName = typeof b.material === "string" ? b.material : "6061";
  } else {
    throw new Error("supply document_id or bar {length_mm, width_mm, thickness_mm}");
  }

  const mat = lookupMaterial(engine, materialName);
  const bar: BarSpec = {
    ...barDims,
    modulus_gpa: mat.modulus_gpa,
    density_kg_m3: mat.density_kg_m3,
  };

  const sampleRate = 44100;
  const durationS = Math.min(Math.max(Number(a.duration_s ?? 2.5), 0.5), 10);
  const strikeFrac = Math.min(Math.max(Number(a.strike_position ?? 0.5), 0.02), 0.98);
  const contactMs = Math.min(Math.max(Number(a.mallet_contact_ms ?? 0.5), 0.05), 10);

  let expected: number | undefined;
  if (typeof a.expect_hz === "number") expected = a.expect_hz;
  else if (typeof a.note === "string") expected = engine.noteToHz(a.note);

  const wantWav = typeof a.wav_filename === "string" && a.wav_filename.length > 0;
  if (wantWav && !/\.wav$/i.test(a.wav_filename as string)) {
    throw new Error("wav_filename must end in .wav");
  }

  const result = engine.simulateStrike({
    bar,
    strike_position: strikeFrac,
    mallet_contact_ms: contactMs,
    duration_s: durationS,
    sample_rate: sampleRate,
    n_modes: 6,
    expected_hz: expected ?? null,
    tolerance_cents: Number(a.tolerance_cents ?? 10),
    include_wav: wantWav,
  }) as KernelStrikeResult;

  const closed = result.closed_form_hz;
  const fem = result.fem_hz;

  const payload: Record<string, unknown> = {
    bar: { ...bar, material: mat.name, material_display: mat.display_name },
    physics: {
      model: "free-free Euler–Bernoulli bar",
      f1_closed_form_hz: closed[0],
      f1_fem_with_holes_hz: fem[0],
      hole_shift_cents: cents(fem[0], closed[0]),
      overtone_ratios: fem.map((f) => f / fem[0]),
      note_overtones:
        "free-free partials are non-harmonic (≈2.76, 5.40, 8.93 × f₁) — that inharmonicity IS the glockenspiel timbre",
    },
    strike: {
      position_frac: strikeFrac,
      mallet_contact_ms: contactMs,
      duration_s: durationS,
      sample_rate: sampleRate,
    },
    modes: result.modes.map((m) => ({
      n: m.n,
      hz: round2(m.hz),
      gain: round4(m.gain),
      q: Math.round(m.q),
      t60_s: round2(m.t60_s),
    })),
    spectrum_peaks: result.spectrum_peaks.map((p) => ({ hz: round2(p.hz), db: round2(p.db) })),
    ...(result.verdict ? { verdict: result.verdict } : {}),
    limits:
      "1-D transverse bending only (no torsional/lateral modes); decay Q is a heuristic (material + cord-at-holes), frequencies are not",
  };

  if (wantWav && result.wav_base64) {
    const bytes = Buffer.from(result.wav_base64, "base64");
    if (isRemoteDeployment()) {
      const cap = maxInlineExportBytes();
      if (bytes.length > cap) {
        throw new Error(
          `WAV is ${bytes.length} bytes — over the ${cap} byte inline cap; lower duration_s`,
        );
      }
      payload.wav = {
        filename: a.wav_filename,
        bytes: bytes.length,
        data_base64: result.wav_base64,
      };
    } else {
      const path = resolveWithinRoot(
        a.wav_filename as string,
        process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd(),
      );
      writeFileSync(path, bytes);
      payload.wav = { path, bytes: bytes.length };
    }
  }

  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}

const round2 = (x: number) => Math.round(x * 100) / 100;
const round4 = (x: number) => Math.round(x * 10000) / 10000;

// ─── ToolDef registry entry ───────────────────────────────────────────────

import { behavior, type ToolDef } from "./tool-def.js";

/** Tool table contributed to the server assembler. */
export const toolDefs: ToolDef[] = [
  {
    name: "simulate_strike",
    pack: "sheet_metal",
    description:
      "Hear a part before it's cut: simulate a mallet strike on a flat free-free bar (glockenspiel/vibraphone bar) and verify its pitch. Modal frequencies from BOTH the closed-form Euler–Bernoulli model and a hole-aware 1-D FEM (cord holes change A(x), I(x)); strike-excited modal synthesis (mode shape at strike point × half-sine mallet spectrum, Q-based decay) → optional 44.1 kHz WAV → FFT peak extraction → cents-error verdict vs `note`/`expect_hz`. Takes a flat sheet_metal_create `document_id` (dims, material, holes read from the session) or an explicit `bar` spec. E/ρ come from the kernel material registry. The `modes` table {hz, gain, q} is compact enough to drive client-side Web Audio synthesis.",
    inputSchema: simulateStrikeSchema,
    handler: (a, c) => simulateStrike(a, c.engine),
    behavior: behavior({}),
  },
];
