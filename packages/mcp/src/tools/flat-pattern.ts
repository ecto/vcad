/**
 * `flat_pattern_from_solid` — the route from "I modelled this part as a plate
 * solid" to "here is the DXF to cut it".
 *
 * The rest of the sheet-metal surface (`sheet_metal_create` → `_unfold`)
 * requires the part to have been *authored* through the sheet-metal ops. Real
 * designs don't work that way: a bracket gets modelled as an extruded sketch
 * or a boolean, verified in an assembly, and only then needs a flat pattern.
 * Redrawing it in a second representation is how a model and its fabrication
 * data drift apart.
 *
 * This tool consumes the geometry that already exists — the mechanical
 * counterpart of `board_from_solid`. It recognises constant-thickness walls
 * and the cylindrical bends between them (kernel: `vcad_kernel_sheet::flatten`),
 * emits the profile as a fab-ready DXF plus a bend table, and **verifies** the
 * result by re-extruding it: if the recovered pattern doesn't account for the
 * solid's volume the part wasn't really sheet metal and the call fails rather
 * than emitting a wrong outline.
 *
 * Batch is the default: called without `part_id` it flattens every solid in
 * the document and groups identical parts into one pattern × N, so eight
 * identical femur plates come back as one DXF and a quantity — which is
 * exactly the shape `sheet_metal_nest` and `sheet_metal_cost` want.
 */

import type { Engine, SheetMetalFromSolid } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { getSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

interface FlatPatternGroup {
  pattern_id: string;
  quantity: number;
  parts: Array<{ root_id: number; name?: string }>;
  thickness_mm: number;
  flat: { bbox: number[]; area_mm2: number; width_mm: number; height_mm: number };
  panels: SheetMetalFromSolid["panels"];
  bend_table: Array<{
    bend: number;
    angle_deg: number;
    radius_mm: number;
    length_mm: number;
    direction: "up" | "down";
    k_factor: number;
    allowance_mm: number;
    line: number[][];
  }>;
  verification: {
    solid_volume_mm3: number;
    recovered_volume_mm3: number;
    error_frac: number;
    status: "verified";
  };
  violations: SheetMetalFromSolid["violations"];
  warnings: string[];
  dxf?: string;
}

/** Rounded to a fixed number of decimals, with `-0` normalised away. */
function q(v: number, dp = 3): number {
  const f = 10 ** dp;
  const r = Math.round(v * f) / f;
  return r === 0 ? 0 : r;
}

/**
 * Translation/rotation-invariant, mirror-*sensitive* signature of a closed
 * ring: the cyclic sequence of `(edge length, turn angle)` reduced to its
 * lexicographically smallest rotation.
 *
 * Mirror sensitivity is the point — a left-hand and a right-hand bracket have
 * identical areas, bboxes and hole counts, and calling them "one pattern × 2"
 * would send the shop half the parts it needs, backwards.
 */
function ringSignature(ring: Array<[number, number]>): string {
  const n = ring.length;
  if (n < 3) return "";
  const terms: string[] = [];
  for (let i = 0; i < n; i++) {
    const a = ring[(i + n - 1) % n];
    const b = ring[i];
    const c = ring[(i + 1) % n];
    const e1 = [b[0] - a[0], b[1] - a[1]];
    const e2 = [c[0] - b[0], c[1] - b[1]];
    const len = Math.hypot(e2[0], e2[1]);
    const turn = Math.atan2(
      e1[0] * e2[1] - e1[1] * e2[0],
      e1[0] * e2[0] + e1[1] * e2[1],
    );
    terms.push(`${q(len, 2)}:${q(turn, 3)}`);
  }
  let best: string | null = null;
  for (let r = 0; r < n; r++) {
    const cand = terms.slice(r).concat(terms.slice(0, r)).join(",");
    if (best === null || cand < best) best = cand;
  }
  return best ?? "";
}

/** Signature of a whole flat pattern: profile + holes + bends + thickness. */
function patternSignature(r: SheetMetalFromSolid): string {
  const sil = r.flatPattern.silhouette_2d ?? [];
  const outer = ringSignature((sil[0] ?? []) as Array<[number, number]>);
  const holes = sil
    .slice(1)
    .map((h) => ringSignature(h as Array<[number, number]>))
    .sort()
    .join("|");
  const bends = r.bends
    .map((b) => `${q(b.angle_deg, 2)}/${q(b.radius, 3)}/${q(b.length, 2)}/${b.direction}`)
    .sort()
    .join("|");
  return `t${q(r.thickness)}|${outer}|H:${holes}|B:${bends}`;
}

/** Solid parts of a document, paired with their evaluated meshes. */
function solidParts(doc: Document, engine: Engine) {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  const out: Array<{
    rootId: number;
    name?: string;
    mesh: { positions: ArrayLike<number>; indices: ArrayLike<number> };
  }> = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const rootId = visibleRoots[i].root;
    const node = doc.nodes[String(rootId)];
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    out.push({ rootId, name: node?.name ?? undefined, mesh });
  }
  return out;
}

export const flatPatternFromSolidSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "CAD session id (open_document / create_cad_loon) holding the solid(s).",
    },
    part_id: {
      type: "string" as const,
      description:
        "Root id of a single part to flatten (from `read`). Omit to flatten every solid part in the document and group identical parts into one pattern × quantity.",
    },
    material: {
      type: "string" as const,
      description:
        'Material key for K-factor lookup (e.g. "al-soft", "steel-mild"). Default "al-soft". See sheet_metal_materials.',
    },
    manual_k: {
      type: "number" as const,
      description: "Override the K-factor for every recovered bend (skips the bend table).",
    },
    shop_profile: {
      type: "string" as const,
      description:
        'Fab-service catalog id (e.g. "sendcutsend") to run manufacturability against. Default: generic shop.',
    },
    volume_tolerance: {
      type: "number" as const,
      description:
        "Max relative volume error the round-trip check accepts (default 0.02). Re-extruding the emitted profile by the detected thickness must reproduce the solid's volume; a mismatch fails the part.",
    },
    include_dxf: {
      type: "boolean" as const,
      description: "Include the DXF string per pattern. Default true.",
    },
  },
  required: ["document_id"],
};

export function flatPatternFromSolid(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  const doc = getSession(String(a.document_id ?? ""));
  const includeDxf = a.include_dxf !== false;
  const options = {
    material: typeof a.material === "string" ? a.material : undefined,
    manualK: typeof a.manual_k === "number" ? a.manual_k : undefined,
    shopProfile: typeof a.shop_profile === "string" ? a.shop_profile : undefined,
    volumeTolFrac:
      typeof a.volume_tolerance === "number" ? a.volume_tolerance : undefined,
  };

  const all = solidParts(doc, engine);
  if (all.length === 0) {
    throw new Error("Document has no solid parts to flatten");
  }
  const partId = a.part_id !== undefined ? String(a.part_id) : undefined;
  const targets = partId
    ? all.filter((p) => String(p.rootId) === partId)
    : all;
  if (targets.length === 0) {
    throw new Error(
      `No part with id "${partId}". Available: ${all
        .map((c) => `${c.rootId}${c.name ? ` (${c.name})` : ""}`)
        .join(", ")}`,
    );
  }

  const groups = new Map<string, FlatPatternGroup>();
  const failed: Array<{ root_id: number; name?: string; reason: string }> = [];

  for (const part of targets) {
    let r: SheetMetalFromSolid;
    try {
      r = engine.flattenSolidToSheetMetal(part.mesh, options);
    } catch (e) {
      failed.push({
        root_id: part.rootId,
        ...(part.name ? { name: part.name } : {}),
        reason: e instanceof Error ? e.message : String(e),
      });
      continue;
    }
    const sig = patternSignature(r);
    const existing = groups.get(sig);
    if (existing) {
      existing.quantity += 1;
      existing.parts.push({
        root_id: part.rootId,
        ...(part.name ? { name: part.name } : {}),
      });
      continue;
    }
    const [minX, minY, maxX, maxY] = r.flatPattern.bbox;
    const creases = r.flatPattern.creases ?? [];
    groups.set(sig, {
      pattern_id: `pattern-${groups.size + 1}`,
      quantity: 1,
      parts: [{ root_id: part.rootId, ...(part.name ? { name: part.name } : {}) }],
      thickness_mm: r.thickness,
      flat: {
        bbox: r.flatPattern.bbox,
        area_mm2: r.flatPattern.area_mm2,
        width_mm: Math.abs(maxX - minX),
        height_mm: Math.abs(maxY - minY),
      },
      panels: r.panels,
      bend_table: r.bends.map((b, i) => ({
        bend: b.bend,
        angle_deg: b.angle_deg,
        radius_mm: b.radius,
        length_mm: b.length,
        direction: b.direction,
        k_factor: b.k_factor,
        allowance_mm:
          (b.angle_deg * Math.PI) / 180 * (b.radius + b.k_factor * r.thickness),
        line: creases[i] ? (creases[i].line as unknown as number[][]) : [],
      })),
      verification: {
        solid_volume_mm3: r.solidVolumeMm3,
        recovered_volume_mm3: r.recoveredVolumeMm3,
        error_frac: r.volumeErrorFrac,
        status: "verified",
      },
      violations: r.violations,
      warnings: r.warnings,
      ...(includeDxf ? { dxf: r.dxf } : {}),
    });
  }

  const patterns = [...groups.values()];
  if (patterns.length === 0) {
    throw new Error(
      `No part in this document is constant-thickness sheet. ${failed
        .map((f) => `${f.name ?? f.root_id}: ${f.reason}`)
        .join("; ")}`,
    );
  }
  const totalParts = patterns.reduce((s, p) => s + p.quantity, 0);
  const bends = patterns.reduce((s, p) => s + p.bend_table.length * p.quantity, 0);

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(
          {
            success: true,
            patterns,
            ...(failed.length > 0 ? { not_sheet_metal: failed } : {}),
            nest_input: patterns.map((p) => ({
              name: p.pattern_id,
              width_mm: p.flat.width_mm,
              height_mm: p.flat.height_mm,
              quantity: p.quantity,
            })),
            summary: `${patterns.length} unique pattern(s) covering ${totalParts} part(s), ${bends} bend(s) total${
              failed.length > 0 ? `; ${failed.length} part(s) are not sheet metal` : ""
            }. Every pattern is volume-verified against its solid.`,
            note: "Each DXF is a fab-ready merged silhouette in millimetres: one closed exterior polyline + hole loops on CUT, DASHED bend centerlines on BEND_UP/BEND_DOWN. DXF carries no bend angles — use `bend_table` (angle, inside radius, direction, allowance, crease line) when entering them in the fab's UI. Hand `nest_input` to sheet_metal_nest, then sheet_metal_cost, for a quote.",
          },
          null,
          2,
        ),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "flat_pattern_from_solid",
    pack: "sheet_metal",
    description:
      "Emit the flat pattern (DXF + bend table) for a part that was modelled as an ordinary solid — extruded sketch, boolean, imported STEP — with no sheet-metal authoring required. The mechanical counterpart of board_from_solid: it detects the constant-thickness walls and the cylindrical bends between them, returns the cut profile (outer boundary + every interior hole) with the detected thickness, and a bend table (line positions, angles, direction, allowance) for bent parts. Called without part_id it batches the whole document and groups identical parts into one pattern × quantity (mirror images stay separate). Fails closed: re-extruding the emitted profile by the detected thickness must reproduce the solid's volume, so a part that isn't really constant-thickness sheet is reported as such instead of yielding a wrong outline.",
    inputSchema: flatPatternFromSolidSchema,
    handler: (a, c) => flatPatternFromSolid(a, c.engine),
    behavior: behavior({ geometry: true }),
  },
];
