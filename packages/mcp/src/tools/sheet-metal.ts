/**
 * MCP tools for sheet-metal — the AI-native surface from the spec's
 * "manufacturability as a queryable interface" bet.
 *
 * `sheet_metal_create` builds a sheet-metal part (base flange + a chain of
 * edge flanges) as a session document and returns its summary + ambient
 * DFM. `sheet_metal_unfold` returns the flat pattern and a layered DXF
 * ready for a laser bureau. `sheet_metal_check` re-runs manufacturability
 * against a caller-supplied shop profile. Together they close the agent
 * loop: create → check → adjust → re-check → unfold.
 *
 * Session-document based (like `inspect_cad` / `export_cad`) so the same
 * `document_id` works with the rest of the MCP surface — `export_cad`,
 * `open_in_browser`, `inspect_cad` all operate on the part this creates.
 *
 * `cost` and `suggest_fix` from the spec are deferred: costing needs the
 * process-cost model wired to the sheet IR, and fix synthesis needs IR-edit
 * primitives. The structured `sheet_metal_check` output is already enough
 * for an agent to self-heal by editing the chain and re-checking.
 */

import type {
  Engine,
  SheetMetalRendered,
  SheetMetalShopProfile,
} from "@vcad/engine";
import { createDocument } from "@vcad/ir";
import type { Document, Node, SheetMetalDirection } from "@vcad/ir";
import { getSession, registerSession } from "./session.js";

interface FlangeSpec {
  edge_index: number;
  length: number;
  angle?: number;
  radius?: number;
  direction?: SheetMetalDirection;
  panel_id?: number;
  manual_k?: number;
}

/** Build a sheet-metal IR document: base flange node + a chain of edge
 *  flange nodes, each parented to the previous. */
function buildSheetMetalDoc(
  base: { width: number; depth: number; thickness: number; material: string },
  flanges: FlangeSpec[],
): Document {
  const doc = createDocument();
  const nodes: Record<string, Node> = {};
  nodes["0"] = {
    id: 0,
    name: "Base flange",
    op: {
      type: "SheetMetalBaseFlangeRect",
      width: base.width,
      depth: base.depth,
      thickness: base.thickness,
      material: base.material,
    },
  };
  let parent = 0;
  flanges.forEach((f, i) => {
    const id = i + 1;
    nodes[String(id)] = {
      id,
      name: `Edge flange ${id}`,
      op: {
        type: "SheetMetalEdgeFlange",
        parent,
        panel_id: f.panel_id ?? 0,
        edge_index: f.edge_index,
        length: f.length,
        angle: f.angle ?? Math.PI / 2,
        radius: f.radius ?? base.thickness,
        direction: f.direction ?? "Up",
        ...(f.manual_k !== undefined ? { manual_k: f.manual_k } : {}),
      },
    };
    parent = id;
  });
  doc.nodes = nodes;
  doc.roots = [{ root: parent, material: "default" }];
  return doc;
}

function renderedOf(engine: Engine, doc: Document): SheetMetalRendered {
  const scene = engine.evaluate(doc);
  for (const part of scene.parts) {
    if (part.sheetMetal) return part.sheetMetal as SheetMetalRendered;
  }
  throw new Error(
    "document has no sheet-metal part (evaluation produced no sheetMetal bundle)",
  );
}

function textResult(payload: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  return {
    content: [{ type: "text", text: JSON.stringify(payload, null, 2) }],
  };
}

// ─── sheet_metal_create ───────────────────────────────────────────────────

export const sheetMetalCreateSchema = {
  type: "object" as const,
  properties: {
    width: {
      type: "number" as const,
      description: "Base-flange width (mm), along +X.",
    },
    depth: {
      type: "number" as const,
      description: "Base-flange depth (mm), along +Y.",
    },
    thickness: {
      type: "number" as const,
      description: "Material thickness (mm). Also the default bend radius.",
    },
    material: {
      type: "string" as const,
      description:
        'Material key for K-factor lookup, e.g. "Al-soft", "steel-mild". Default "Al-soft".',
    },
    flanges: {
      type: "array" as const,
      description:
        "Optional edge flanges, applied in order. Each is bent off an edge of the panel created so far.",
      items: {
        type: "object" as const,
        properties: {
          edge_index: {
            type: "number" as const,
            description:
              "Which edge of the panel to flange (0 = the y=0 edge, CCW from there).",
          },
          length: {
            type: "number" as const,
            description: "Flange length (mm) perpendicular to the hinge.",
          },
          angle: {
            type: "number" as const,
            description: "Bend angle in radians. Default π/2 (90°).",
          },
          radius: {
            type: "number" as const,
            description: "Inside bend radius (mm). Default = thickness.",
          },
          direction: {
            type: "string" as const,
            enum: ["Up", "Down"],
            description: "Fold direction. Default Up.",
          },
          panel_id: {
            type: "number" as const,
            description: "Panel to flange off (0 = base). Default 0.",
          },
          manual_k: {
            type: "number" as const,
            description: "Optional K-factor override (skips bend-table lookup).",
          },
        },
        required: ["edge_index", "length"],
      },
    },
  },
  required: ["width", "depth", "thickness"],
};

export function sheetMetalCreate(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  const base = {
    width: Number(a.width),
    depth: Number(a.depth),
    thickness: Number(a.thickness),
    material: typeof a.material === "string" ? a.material : "Al-soft",
  };
  const flanges = Array.isArray(a.flanges)
    ? (a.flanges as FlangeSpec[])
    : [];
  const doc = buildSheetMetalDoc(base, flanges);
  const rendered = renderedOf(engine, doc);
  const documentId = registerSession(doc);
  const errors = rendered.violations.filter(
    (v) => v.severity === "Error",
  ).length;
  return textResult({
    document_id: documentId,
    model: rendered.model,
    flat: {
      bbox: rendered.flatPattern.bbox,
      area_mm2: rendered.flatPattern.area_mm2,
    },
    violations: rendered.violations,
    summary: `${rendered.model.panel_count} panels, ${rendered.model.bend_count} bends; ${errors} error(s), ${rendered.violations.length - errors} warning(s) vs. generic shop. Use sheet_metal_check with a shop_profile to verify against real capabilities, sheet_metal_unfold for the flat/DXF.`,
  });
}

// ─── sheet_metal_unfold ───────────────────────────────────────────────────

export const sheetMetalUnfoldSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from sheet_metal_create.",
    },
    include_dxf: {
      type: "boolean" as const,
      description:
        "Include the layered DXF (CUT / BEND_UP / BEND_DOWN) string. Default true.",
    },
  },
  required: ["document_id"],
};

export function sheetMetalUnfold(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  const doc = getSession(String(a.document_id ?? ""));
  const rendered = renderedOf(engine, doc);
  const includeDxf = a.include_dxf !== false;
  return textResult({
    flat_pattern: rendered.flatPattern,
    ...(includeDxf ? { dxf: rendered.dxf } : {}),
  });
}

// ─── sheet_metal_check ────────────────────────────────────────────────────

export const sheetMetalCheckSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from sheet_metal_create.",
    },
    shop_profile: {
      type: "object" as const,
      description:
        "Optional shop capabilities. Field-tolerant: omitted keys fall back to the generic shop. Keys: name, max_bend_length_mm, min_bend_radius_ratio, min_flange_height_mm, min_hole_to_bend_mm, min_distance_between_bends_mm.",
    },
  },
  required: ["document_id"],
};

export function sheetMetalCheck(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  const doc = getSession(String(a.document_id ?? ""));
  const shop =
    a.shop_profile && typeof a.shop_profile === "object"
      ? (a.shop_profile as Partial<SheetMetalShopProfile>)
      : undefined;
  const result = engine.checkSheetMetal(
    doc,
    shop as SheetMetalShopProfile | undefined,
  );
  if (!result) {
    throw new Error("document has no sheet-metal part");
  }
  const errors = result.violations.filter(
    (v) => v.severity === "Error",
  ).length;
  return textResult({
    shop: result.shop,
    violations: result.violations,
    shop_ready: result.violations.length === 0,
    error_count: errors,
    warning_count: result.violations.length - errors,
  });
}
