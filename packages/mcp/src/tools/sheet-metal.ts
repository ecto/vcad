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
  SheetMetalCostRates,
  SheetMetalRendered,
  SheetMetalShopProfile,
} from "@vcad/engine";
import { createDocument } from "@vcad/ir";
import type {
  Document,
  Node,
  SheetMetalDirection,
  SheetMetalHemKind,
} from "@vcad/ir";
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

interface HemSpec {
  edge_index: number;
  length: number;
  kind?: SheetMetalHemKind;
  gap?: number;
  direction?: SheetMetalDirection;
  panel_id?: number;
}

interface JogSpec {
  edge_index: number;
  offset: number;
  length: number;
  radius?: number;
  direction?: SheetMetalDirection;
  panel_id?: number;
}

/** Build a sheet-metal IR document: base flange node + ordered edge
 *  flanges and hems, each parented to the previous. */
function buildSheetMetalDoc(
  base: { width: number; depth: number; thickness: number; material: string },
  flanges: FlangeSpec[],
  hems: HemSpec[],
  jogs: JogSpec[],
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
  let nextId = 1;
  flanges.forEach((f) => {
    const id = nextId++;
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
  hems.forEach((h) => {
    const id = nextId++;
    nodes[String(id)] = {
      id,
      name: `Hem ${id}`,
      op: {
        type: "SheetMetalHem",
        parent,
        panel_id: h.panel_id ?? 0,
        edge_index: h.edge_index,
        kind: h.kind ?? "Closed",
        length: h.length,
        gap: h.gap ?? 0,
        direction: h.direction ?? "Up",
      },
    };
    parent = id;
  });
  jogs.forEach((j) => {
    const id = nextId++;
    nodes[String(id)] = {
      id,
      name: `Jog ${id}`,
      op: {
        type: "SheetMetalJog",
        parent,
        panel_id: j.panel_id ?? 0,
        edge_index: j.edge_index,
        offset: j.offset,
        length: j.length,
        radius: j.radius ?? base.thickness,
        direction: j.direction ?? "Up",
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
    jogs: {
      type: "array" as const,
      description:
        "Optional jogs (Z-shaped offsets) applied after all flanges and hems. Two 90° bends in series.",
      items: {
        type: "object" as const,
        properties: {
          edge_index: {
            type: "number" as const,
            description: "Which edge of the panel to jog from.",
          },
          offset: {
            type: "number" as const,
            description: "Vertical offset (mm) between parent and tail planes.",
          },
          length: {
            type: "number" as const,
            description: "Tail panel length (mm) past the second bend.",
          },
          radius: {
            type: "number" as const,
            description: "Inside bend radius for both bends. Default = thickness.",
          },
          direction: {
            type: "string" as const,
            enum: ["Up", "Down"],
            description: "Direction of the first fold. Default Up.",
          },
          panel_id: {
            type: "number" as const,
            description: "Panel to jog from. Default 0.",
          },
        },
        required: ["edge_index", "offset", "length"],
      },
    },
    hems: {
      type: "array" as const,
      description:
        "Optional hems (180° folds) applied after all flanges. Use for edge stiffening / burr removal.",
      items: {
        type: "object" as const,
        properties: {
          edge_index: {
            type: "number" as const,
            description: "Which edge of the panel to hem.",
          },
          length: {
            type: "number" as const,
            description:
              "Back-flange length (mm) — how far the hem extends past the fold.",
          },
          kind: {
            type: "string" as const,
            enum: ["Closed", "Open"],
            description:
              "Closed = faces touch; Open = gap between parent and back-flange. Default Closed.",
          },
          gap: {
            type: "number" as const,
            description:
              "Gap (mm) between parent and back-flange. Required for Open hems; ignored for Closed.",
          },
          direction: {
            type: "string" as const,
            enum: ["Up", "Down"],
            description: "Fold direction. Default Up.",
          },
          panel_id: {
            type: "number" as const,
            description: "Panel to hem from. Default 0 (base).",
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
  const hems = Array.isArray(a.hems) ? (a.hems as HemSpec[]) : [];
  const jogs = Array.isArray(a.jogs) ? (a.jogs as JogSpec[]) : [];
  const doc = buildSheetMetalDoc(base, flanges, hems, jogs);
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

// ─── sheet_metal_materials ────────────────────────────────────────────────

export const sheetMetalMaterialsSchema = {
  type: "object" as const,
  properties: {},
};

export function sheetMetalMaterials(
  _input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const materials = engine.getSheetMetalMaterials();
  return textResult({
    materials,
    note: "Pass any `name` (e.g. `\"al-soft\"`, `\"steel-mild\"`) as the `material` field of `sheet_metal_create`. Aliases like `\"aluminum\"`, `\"stainless\"`, `\"6061-T6\"` also resolve.",
  });
}

// ─── sheet_metal_bend_table ───────────────────────────────────────────────

export const sheetMetalBendTableSchema = {
  type: "object" as const,
  properties: {},
};

export function sheetMetalBendTable(
  _input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const table = engine.getSheetMetalBendTable();
  return textResult({
    table,
    note: "K-factors are interpolated by closest R/t for the chosen material. To override K for a specific bend, pass `manual_k` on that flange in sheet_metal_create.",
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

// ─── sheet_metal_suggest_fix ──────────────────────────────────────────────

export const sheetMetalSuggestFixSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from sheet_metal_create.",
    },
    shop_profile: {
      type: "object" as const,
      description:
        "Optional shop profile (same shape as sheet_metal_check). Defaults to generic.",
    },
    violation_index: {
      type: "number" as const,
      description:
        "Index into the violations array (from sheet_metal_check). Omit to get suggestions for every violation.",
    },
  },
  required: ["document_id"],
};

/**
 * Translate a structured violation into a concrete fix the agent can act
 * on. Closes the spec's AI self-heal loop: sheet_metal_check finds
 * problems; sheet_metal_suggest_fix names the parameter change that
 * resolves each one. The agent then re-runs sheet_metal_create with the
 * adjusted spec and re-checks.
 */
export function sheetMetalSuggestFix(
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
  const idxRaw = a.violation_index;
  const violations =
    typeof idxRaw === "number"
      ? [result.violations[Math.floor(idxRaw)]].filter(
          (v): v is (typeof result.violations)[number] => Boolean(v),
        )
      : result.violations;
  const suggestions = violations.map((v, i) => ({
    index: typeof idxRaw === "number" ? Math.floor(idxRaw) : i,
    violation: v,
    fix: suggestFix(v),
  }));
  return textResult({
    shop: result.shop,
    count: suggestions.length,
    suggestions,
    note: "Apply by re-issuing sheet_metal_create with the adjusted spec (parameters indicated by each `fix.action` + `fix.value`). Then call sheet_metal_check to verify.",
  });
}

type FixAction =
  | "increase_radius"
  | "increase_flange_length"
  | "shorten_or_split_bend"
  | "move_hole_or_clearance"
  | "separate_bends"
  | "manual";

interface Fix {
  action: FixAction;
  description: string;
  /** Structured fields the agent reads to translate to a chain edit. */
  [field: string]: unknown;
}

function suggestFix(v: SheetMetalViolationLike): Fix {
  const d = v.detail as Record<string, unknown>;
  switch (d.kind) {
    case "BendRadiusBelowMinimum": {
      const required = Number(d.required_mm);
      const source = String(d.source ?? "Material");
      const material = String(d.material ?? "");
      const reason =
        source === "Material" && material
          ? `${material} cracks below this`
          : source === "Material"
            ? "material cracks below this"
            : "shop tooling can't form tighter";
      return {
        action: "increase_radius",
        bend_id: d.bend_id,
        new_radius_mm: required,
        description: `Increase bend #${d.bend_id} inside radius to at least ${required.toFixed(2)} mm (${reason}).`,
      };
    }
    case "FlangeBelowMinHeight": {
      const required = Number(d.required_mm);
      return {
        action: "increase_flange_length",
        bend_id: d.bend_id,
        panel_id: d.panel_id,
        new_length_mm: required,
        description: `Lengthen the flange off bend #${d.bend_id} to at least ${required.toFixed(2)} mm so the brake can grip it.`,
      };
    }
    case "BendExceedsBrakeCapacity": {
      const max = Number(d.required_mm);
      return {
        action: "shorten_or_split_bend",
        bend_id: d.bend_id,
        max_length_mm: max,
        description: `Bend #${d.bend_id} is ${Number(d.actual_mm).toFixed(0)} mm — over the ${max.toFixed(0)} mm brake. Reduce the part width, split into two bends with relief, or move to a longer brake.`,
      };
    }
    case "HoleTooCloseToBend": {
      const required = Number(d.required_mm);
      return {
        action: "move_hole_or_clearance",
        bend_id: d.bend_id,
        hole_index: d.hole_index,
        required_clearance_mm: required,
        description: `Move hole #${d.hole_index} at least ${required.toFixed(2)} mm from bend #${d.bend_id} (currently ${Number(d.actual_mm).toFixed(2)} mm). Otherwise the hole deforms into a slot.`,
      };
    }
    case "BendsTooClose": {
      const required = Number(d.required_mm);
      return {
        action: "separate_bends",
        bend_id_a: d.bend_id_a,
        bend_id_b: d.bend_id_b,
        required_distance_mm: required,
        description: `Move bends #${d.bend_id_a} and #${d.bend_id_b} to at least ${required.toFixed(2)} mm apart (currently ${Number(d.actual_mm).toFixed(2)} mm) so the back-gauge can register.`,
      };
    }
    default:
      return {
        action: "manual",
        description: `No structured fix for ${String(d.kind)} — manual review.`,
      };
  }
}

interface SheetMetalViolationLike {
  detail: Record<string, unknown> & { kind: string };
}

// ─── sheet_metal_cost ─────────────────────────────────────────────────────

export const sheetMetalCostSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from sheet_metal_create.",
    },
    quantity: {
      type: "number" as const,
      description:
        "Job quantity for amortizing setup. Default 1. Clamped to >= 1.",
    },
    rates: {
      type: "object" as const,
      description:
        "Optional shop pricing — field-tolerant (omit keys to use generic). Keys: currency, material_usd_per_kg, cut_usd_per_m, pierce_usd_each, bend_usd_each, setup_usd, markup_pct.",
    },
  },
  required: ["document_id"],
};

export function sheetMetalCost(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  const doc = getSession(String(a.document_id ?? ""));
  const qty = Math.max(1, Math.floor(Number(a.quantity ?? 1)));
  const rates =
    a.rates && typeof a.rates === "object"
      ? (a.rates as Partial<SheetMetalCostRates>)
      : undefined;
  const result = engine.costSheetMetal(
    doc,
    rates as SheetMetalCostRates | undefined,
    qty,
  );
  if (!result) {
    throw new Error("document has no sheet-metal part");
  }
  return textResult({
    breakdown: result.breakdown,
    rates: result.rates,
    summary: `${result.breakdown.currency} ${result.breakdown.total_each.toFixed(2)} each @ qty ${result.breakdown.quantity} (mass ${result.breakdown.mass_kg_each.toFixed(3)} kg, ${result.breakdown.cut_length_m.toFixed(2)} m cut, ${result.breakdown.bends} bend(s)).`,
  });
}
