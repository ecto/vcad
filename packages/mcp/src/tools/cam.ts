/**
 * The `cam` tool pack — CNC milling for agents.
 *
 * On the first real cut (2026-09-17) an agent had to take over the user's
 * screen and press "Export job", because CAM existed in the kernel, in the C
 * ABI and in the app, and nowhere an agent could reach. These six tools are
 * that door.
 *
 * Every one of them is a thin shell over `vcad-cam-api` through the kernel
 * WASM — the *same* request schema the native app posts through its C ABI. A
 * second implementation of "is this job safe?" would be a second answer, and
 * the whole point of the verification oracle is that there is one.
 *
 * ── The contract these tools advertise ─────────────────────────────────────
 *
 *  1. **Verify by default.** `cam_job` replays the program it just posted
 *     against the part it is meant to make, before handing it over.
 *  2. **Blocked means do not cut.** A refused job comes back with
 *     `blocked: true`, the checks that failed, and **no G-code anywhere in the
 *     result** — there is nothing to export by accident.
 *  3. **Physical predictions are provisional.** Cycle time, feeds and speeds,
 *     and cutter fit are computed, not measured. They stay predictions until a
 *     part comes off the machine and is measured.
 *
 * ── Results are compact by default ─────────────────────────────────────────
 *
 * Agents read these. Every tool answers summaries, counts and worst values;
 * `detail: "full"` attaches the arrays — contour points, every check's detail,
 * the G-code text inline.
 */

import type { Document } from "@vcad/ir";
import { resetKernelWasm, type Engine } from "@vcad/engine";
import { getSession } from "./session-core.js";
import { storeArtifact } from "./artifact-store.js";
import { depositFrom, depositClaimReport } from "./claim-registry.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { err, ok, type ToolResult } from "./tool-result.js";

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

type Json = Record<string, unknown>;

const num = (v: unknown): number | undefined =>
  typeof v === "number" && Number.isFinite(v) ? v : undefined;

/** Round for display: enough digits for a machinist, not enough to be noise. */
const mm = (v: unknown): number | undefined => {
  const n = num(v);
  return n === undefined ? undefined : Number(n.toFixed(4));
};

const isFull = (args: Json): boolean => args.detail === "full";

/**
 * The scene index of a named part.
 *
 * `engine.evaluate` pairs `scene.parts[i]` with the i-th *visible* root, and
 * that index is what the kernel's own sectioning entry point wants — it
 * re-evaluates the document itself so it can reach the B-rep, which the
 * evaluated scene's mesh has already lost.
 */
function partIndex(
  doc: Document,
  engine: Engine,
  wanted: string | undefined,
): { index: number; name: string } {
  const scene = engine.evaluate(doc);
  const visibleRoots = doc.roots.filter((e) => e.visible !== false);
  const nameOf = (i: number) =>
    doc.nodes[String(visibleRoots[i]?.root)]?.name ?? `part ${i}`;
  if (wanted === undefined || wanted === "") {
    if (scene.parts.length === 0) {
      throw new Error("this document has no parts to machine");
    }
    if (scene.parts.length > 1) {
      const names = scene.parts.map((_, i) => nameOf(i)).join(", ");
      throw new Error(
        `this document has ${scene.parts.length} parts, so \`part\` is required. They are: ${names}`,
      );
    }
    return { index: 0, name: nameOf(0) };
  }
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const root = visibleRoots[i];
    if (String(root.root) === wanted || doc.nodes[String(root.root)]?.name === wanted) {
      return { index: i, name: nameOf(i) };
    }
  }
  const names = scene.parts.map((_, i) => nameOf(i)).join(", ");
  throw new Error(
    `part "${wanted}" not found — pass a root part id or exact name. This document has: ${names}`,
  );
}

/** Section a document part, as the kernel does it: raw tessellation, B-rep first. */
function sectionPart(
  engine: Engine,
  doc: Document,
  index: number,
  args: Json,
): Json {
  const z = num(args.z);
  const autoZ = args.auto_z === undefined ? z === undefined : args.auto_z === true;
  const options: Json = {};
  for (const key of [
    "weld_tolerance",
    "heal_tolerance",
    "prismatic_tolerance",
    "circle_tolerance",
    "segments",
  ]) {
    if (args[key] !== undefined) options[key] = args[key];
  }
  return engine.camOutlineFromDocument<Json>(doc, index, z ?? 0, autoZ, options);
}

/**
 * A contour named rather than spelled out.
 *
 * `"outer"` is the part's boundary, `"hole:0"` the largest opening, `"hole:1"`
 * the next, and so on — the order `cam_outline` lists them in. Without this an
 * agent has to carry four hundred coordinate pairs from one tool call to the
 * next and hope none of them is dropped on the way.
 */
function resolveContour(
  selector: unknown,
  outline: Json | null,
  what: string,
): number[][] {
  if (Array.isArray(selector)) return selector as number[][];
  if (typeof selector !== "string") {
    throw new Error(
      `${what}: give a closed polyline of [x, y] points, or the name of one from cam_outline ("outer", "hole:0", …).`,
    );
  }
  if (!outline) {
    throw new Error(
      `${what} names the contour "${selector}", but nothing was sectioned to look it up in. Pass \`document_id\` (and \`part\` when the document has several), or give the points.`,
    );
  }
  const regions = (outline.regions as Json[] | undefined) ?? [];
  const region = regions[0];
  if (!region) throw new Error(`${what}: the section found no regions.`);
  if (selector === "outer") return region.outer as number[][];
  const hole = /^hole:(\d+)$/.exec(selector);
  if (hole) {
    const holes = (region.holes as number[][][] | undefined) ?? [];
    const i = Number(hole[1]);
    if (!holes[i]) {
      throw new Error(
        `${what} names "${selector}", but this part has ${holes.length} hole(s).`,
      );
    }
    return holes[i];
  }
  throw new Error(
    `${what}: "${selector}" is not a contour name. Use "outer" or "hole:<n>".`,
  );
}

/** Area of a closed polyline, for ordering holes largest-first. */
function loopArea(points: number[][]): number {
  let a = 0;
  for (let i = 0; i < points.length; i++) {
    const p = points[i];
    const q = points[(i + 1) % points.length];
    a += p[0] * q[1] - q[0] * p[1];
  }
  return Math.abs(a / 2);
}

/** Holes largest-first, so `hole:0` is stably the bore rather than a pilot. */
function sortedOutline(outline: Json): Json {
  const regions = (outline.regions as Json[] | undefined) ?? [];
  for (const region of regions) {
    const holes = region.holes as number[][][] | undefined;
    if (holes) holes.sort((a, b) => loopArea(b) - loopArea(a));
  }
  return outline;
}

/** The section of a part named in the args, or null when none was asked for. */
function outlineFor(
  args: Json,
  engine: Engine,
): { outline: Json; doc: Document; index: number; name: string } | null {
  const documentId = typeof args.document_id === "string" ? args.document_id : "";
  if (!documentId) return null;
  const doc = getSession(documentId);
  const { index, name } = partIndex(
    doc,
    engine,
    typeof args.part === "string" ? args.part : undefined,
  );
  const outline = sectionPart(engine, doc, index, args);
  if (typeof outline.error === "string") {
    throw new TornSolid(outline);
  }
  return { outline: sortedOutline(outline), doc, index, name };
}

/** A section that does not close: the solid is torn, and where. */
class TornSolid extends Error {
  constructor(readonly payload: Json) {
    super(String(payload.error));
    this.name = "TornSolid";
  }
}

/** The one place a thrown handler error becomes a tool result. */
function guard(fn: () => ToolResult): ToolResult {
  try {
    return fn();
  } catch (e) {
    // A kernel trap poisons the WASM instance: every later call into it fails
    // for an unrelated-looking reason. Reset it here so the next tool call
    // gets a fresh module, and say plainly that this is a kernel bug rather
    // than something about the job.
    //
    // The known one: `vcad-eval` budgets its boolean batching with
    // `std::time::Instant::now`, which is not implemented on
    // wasm32-unknown-unknown and panics. A document that reaches that path —
    // a chain of Difference nodes, e.g. a plate with several holes cut one
    // after another — traps. The app never saw it because `evaluateDocument`
    // catches the trap and silently falls back to evaluating in TypeScript;
    // sectioning has no such fallback, because the whole point is to reach the
    // B-rep the TypeScript path does not have.
    if (e instanceof WebAssembly.RuntimeError) {
      resetKernelWasm(`cam: kernel trapped: ${e.message}`);
      return err(
        `The kernel trapped while working on this part (${e.message}). This is a bug in the kernel, not in the job. A document built as a chain of Difference nodes is the known trigger — authoring the cuts as one Difference against a union of the tools avoids it. The WASM instance has been reset, so the next call starts clean.`,
      );
    }
    if (e instanceof TornSolid) {
      const gaps = (e.payload.gaps as Json[] | undefined) ?? [];
      return err(
        JSON.stringify({
          reason: "torn_solid",
          message: e.payload.error,
          mesh_source: e.payload.mesh_source,
          z: e.payload.z,
          gap_count: gaps.length,
          widest_gap_mm: mm(
            gaps.reduce((w, g) => Math.max(w, num(g.distance) ?? 0), 0),
          ),
          gaps: gaps.slice(0, 20),
          fix: "Heal the solid — the outline is a symptom. `heal_tolerance` will close gaps smaller than it, but a gap this wide is geometry the kernel could not resolve, not noise.",
        }),
      );
    }
    return err(e instanceof Error ? e.message : String(e));
  }
}

/** Every verification check, folded to pass/fail with its worst number. */
function checkSummary(verification: Json | undefined, full: boolean): Json {
  if (!verification) return {};
  const names = [
    "gouge",
    "rapids",
    "plunges",
    ["material_left", "check"],
    ["depth", "check"],
    ["tabs", "check"],
    ["envelope", "check"],
    ["loose", "check"],
  ] as Array<string | [string, string]>;
  const out: Json = {};
  for (const entry of names) {
    const [key, sub] = Array.isArray(entry) ? entry : [entry, null];
    const holder = verification[key] as Json | undefined;
    const check = (sub ? (holder?.[sub] as Json | undefined) : holder) ?? undefined;
    if (!check) continue;
    out[key] = check.pass
      ? "pass"
      : {
          failed: true,
          severity: check.severity,
          worst: mm(check.worst),
          note: check.note,
        };
  }
  if (full) out.detail = verification;
  return out;
}

const SAFETY =
  "Verified against the part before it was handed over. `blocked: true` means do not cut. Cycle time, feeds and cutter fit are predictions until a part is measured.";

/**
 * Store the kernel's `claims` block on the session document, when the call
 * named one.
 *
 * This is the step that makes a CAM job *certifiable* rather than merely
 * checked: the verification report says whether the oracle objected now, the
 * deposit says what the job claims about the part and what those claims rest
 * on, so `build_receipt` can re-state them later and `record_measurement` can
 * close the predicted ones. Without a `document_id` there is nowhere to put
 * it and the block still rides in the result, so nothing is lost — only
 * unpersisted.
 *
 * The id is stable per job name, so re-posting the same job replaces its
 * claims instead of stacking a second, contradictory set on the document.
 */
function deposit(
  args: Json,
  block: unknown,
  idPrefix: string,
  label: string,
): Json | undefined {
  const documentId = typeof args.document_id === "string" ? args.document_id : "";
  if (!documentId) return undefined;
  const slug = label.replace(/[^a-z0-9._-]+/gi, "-").toLowerCase() || "default";
  const entry = depositFrom(block, `${idPrefix}:${slug}`, label);
  if (!entry) return undefined;
  depositClaimReport(getSession(documentId), entry);
  const summary = (block as Json).summary;
  return {
    id: entry.id,
    schema: entry.schema,
    document_id: documentId,
    ...(summary ? { summary } : {}),
    next: "build_receipt on this document now carries these claims; record_measurement closes the predicted ones.",
  };
}

// ---------------------------------------------------------------------------
// cam_outline
// ---------------------------------------------------------------------------

const sectionProperties = {
  document_id: {
    type: "string" as const,
    description: "CAD session id holding the part (open_document / create_cad_loon).",
  },
  part: {
    type: "string" as const,
    description:
      "Root part id or exact name. Optional when the document holds one part.",
  },
  z: {
    type: "number" as const,
    description:
      "Height to section at, mm. Omit (or pass auto_z) to use the mid-height of the part's own Z range.",
  },
  auto_z: {
    type: "boolean" as const,
    description: "Section at the part's mid-height, ignoring `z`. Default true when `z` is omitted.",
  },
  heal_tolerance: {
    type: "number" as const,
    description:
      "Close gaps narrower than this when chaining the section, mm (default 1e-3). Raising it hides a torn solid rather than fixing one — the refusal reports where the gaps are.",
  },
  circle_tolerance: {
    type: "number" as const,
    description: "How closely a hole must fit a circle to be reported as one, mm (default 1e-3).",
  },
  prismatic_tolerance: {
    type: "number" as const,
    description:
      "How far sections at different heights may disagree and still count as constant-section, mm (default 0.01).",
  },
  segments: {
    type: "number" as const,
    description: "Curve resolution when the B-rep is tessellated for sectioning (default 64).",
  },
};

export const camOutlineSchema = {
  type: "object" as const,
  required: ["document_id"],
  properties: {
    ...sectionProperties,
    detail: {
      type: "string" as const,
      description:
        "'summary' (default) returns counts, areas, bounds and the detected circles. 'full' also attaches every contour's points and the raw outline document.",
    },
  },
};

export function camOutline(args: Json, engine: Engine): ToolResult {
  return guard(() => {
    const resolved = outlineFor(args, engine);
    if (!resolved) return err("Pass `document_id` (the CAD session holding the part).");
    const { outline, name } = resolved;
    const full = isFull(args);

    const regions = ((outline.regions as Json[] | undefined) ?? []).map((r, i) => {
      const outer = (r.outer as number[][] | undefined) ?? [];
      const holes = (r.holes as number[][][] | undefined) ?? [];
      const region: Json = {
        index: i,
        outer_points: outer.length,
        holes: holes.length,
        area_mm2: mm(r.area),
      };
      if (full) {
        region.outer = outer;
        region.hole_loops = holes;
      }
      return region;
    });

    const circles = ((outline.circles as Json[] | undefined) ?? []).map((c) => ({
      name: `hole:${c.hole}`,
      region: c.region,
      center: (c.center as number[] | undefined)?.map((v) => Number(v.toFixed(4))),
      diameter: mm(c.diameter),
      max_error_mm: mm(c.max_error),
    }));

    const prismatic = outline.prismatic as Json | null;
    const verdict =
      prismatic && typeof prismatic.prismatic === "boolean"
        ? {
            prismatic: prismatic.prismatic,
            tolerance_mm: mm(prismatic.tolerance),
            // The measured disagreement, which is the number that matters when
            // the answer is "no": a contour job cuts one shape all the way
            // down, and this is how wrong that is for this part.
            max_disagreement_mm: mm(prismatic.max_boundary_distance),
            worst_z: mm(prismatic.worst_z),
            note: prismatic.prismatic
              ? "Every sampled height agrees: a 2.5D contour job describes this part."
              : "Sections at different heights disagree by more than the tolerance. A 2.5D contour job will cut one of them and not the others.",
          }
        : (prismatic ?? null);

    const body: Json = {
      ok: true,
      part: name,
      z: mm(outline.z),
      auto_z: outline.auto_z,
      // Which mesh this came from is load-bearing: the export mesh is repaired
      // for printing and can be torn by 0.4 mm where tangent fillets meet, so
      // a contour taken from it is not the wall the cutter should follow.
      mesh_source: outline.mesh_source,
      z_range: (outline.z_range as number[] | undefined)?.map((v) => Number(v.toFixed(4))),
      suggested_stock_thickness_mm: mm(outline.suggested_stock_thickness),
      bounds: (outline.bounds as number[] | undefined)?.map((v) => Number(v.toFixed(4))),
      area_mm2: mm(outline.area),
      regions,
      circles,
      prismatic: verdict,
      healed: outline.healed,
      discarded_slivers: outline.discarded_slivers,
      contour_names: [
        "outer",
        ...(((outline.regions as Json[] | undefined)?.[0]?.holes as unknown[] | undefined) ?? []).map(
          (_, i) => `hole:${i}`,
        ),
      ],
      next: "Name these contours in cam_fit and cam_job (\"outer\", \"hole:0\", …) with the same document_id — the points do not have to travel.",
    };
    if (full) body.outline = outline.outline;
    return ok(body);
  });
}

// ---------------------------------------------------------------------------
// cam_fit
// ---------------------------------------------------------------------------

export const camFitSchema = {
  type: "object" as const,
  required: ["tool_diameter"],
  properties: {
    ...sectionProperties,
    contour: {
      description:
        'The contour to check: a closed polyline of [x, y] points, or a name from cam_outline ("outer", "hole:0", …) resolved against `document_id`.',
    },
    tool_diameter: { type: "number" as const, description: "Cutting diameter, mm." },
    side: {
      type: "string" as const,
      description:
        '"inside" (default — the cutter works within the loop, as in a pocket or a bore) or "outside".',
    },
    detail: {
      type: "string" as const,
      description: "'summary' (default) or 'full' to attach every unreachable region.",
    },
  },
};

export function camFit(args: Json, engine: Engine): ToolResult {
  return guard(() => {
    const resolved = outlineFor(args, engine);
    const contour = resolveContour(
      args.contour ?? "outer",
      resolved?.outline ?? null,
      "cam_fit.contour",
    );
    const out = engine.camFit<Json>({
      contour,
      tool_diameter: args.tool_diameter,
      side: args.side,
      grid: args.grid,
      min_area: args.min_area,
    });
    if (typeof out.error === "string") return err(out.error);

    const report = out.report as Json;
    const unreachable = report.unreachable as Json;
    const body: Json = {
      ok: true,
      fits: out.fits,
      side: out.side,
      tool_diameter: args.tool_diameter,
      // The number that answers "so what should I use?".
      largest_tool_that_fits_mm: mm(out.largest_tool_diameter),
      unreachable_corners: unreachable?.count ?? 0,
      metal_left_mm2: mm(unreachable?.total_area),
      worst_standoff_mm: mm(unreachable?.max_standoff),
      necks: (report.necks as unknown[] | undefined)?.length ?? 0,
      note: out.fits
        ? "This cutter reaches every corner of this contour."
        : `This cutter cannot reach ${unreachable?.count ?? 0} corner(s), leaving up to ${mm(unreachable?.max_standoff)} mm of wall standing. Use a smaller cutter, or accept the corners as drawn.`,
      provisional: "Fit is geometry, not a cut. It says nothing about deflection, chatter or how the tool behaves in this material.",
    };
    if (isFull(args)) body.report = report;
    return ok(body);
  });
}

// ---------------------------------------------------------------------------
// cam_recommend_feeds
// ---------------------------------------------------------------------------

export const camRecommendFeedsSchema = {
  type: "object" as const,
  required: ["material", "tool"],
  properties: {
    material: {
      type: "string" as const,
      description:
        'Material id, e.g. "copper-c110", "brass-c360", "aluminium-6061-t6", "mdf", "fr4-copper-clad". Call with `list: true` to see the table.',
    },
    list: {
      type: "boolean" as const,
      description: "Return the material table instead of a recommendation.",
    },
    op: {
      type: "string" as const,
      description:
        '"slot" (default — a profile cut right through sheet stock is buried on both sides and IS a slot), "profile", "pocket", "drill" or "finish".',
    },
    tool: {
      type: "object" as const,
      description: "{ diameter, flutes?, kind?, flute_length? } in mm.",
      properties: {
        diameter: { type: "number" as const },
        flutes: { type: "number" as const },
        kind: { type: "string" as const },
        flute_length: { type: "number" as const },
      },
    },
    machine: {
      type: "object" as const,
      description:
        '{ class: "hobby" | "benchtop" | "vmc", spindle: "dial" | "controlled", max_feed? }. A machine that does not say gets a dial router, because assuming the S word works when it does not leaves a cutter at the wrong speed with no warning.',
    },
    settings: {
      type: "object" as const,
      description:
        "{ feed, plunge, rpm, stepdown, stepover? } — pass numbers you already have and they are checked against the material instead of a recommendation being made.",
    },
    detail: { type: "string" as const, description: "'summary' (default) or 'full'." },
  },
};

export function camRecommendFeeds(args: Json, engine: Engine): ToolResult {
  return guard(() => {
    if (args.list === true) {
      const table = engine.camMaterials<Json>();
      const materials = ((table.materials as Json[] | undefined) ?? []).map((m) => ({
        id: m.id,
        name: m.name,
        coolant: m.coolant,
      }));
      return ok({ ok: true, materials, operations: table.operations, machine_classes: table.machine_classes, spindles: table.spindles });
    }

    const request = {
      material: args.material,
      op: args.op,
      tool: args.tool,
      machine: args.machine,
      settings: args.settings,
    };

    // Numbers in hand get checked; no numbers means make some.
    if (args.settings) {
      const out = engine.camCheckFeeds<Json>(request);
      if (typeof out.error === "string") return err(out.error);
      const notes = (out.notes as Json[] | undefined) ?? [];
      return ok({
        ok: true,
        checked: true,
        acceptable: out.ok,
        worst_level: out.worst_level,
        material: out.material,
        notes: isFull(args)
          ? notes
          : notes.map((n) => ({ level: n.level, text: n.text })),
        provisional: "The table is a starting point from published chiploads, not a measurement of your machine.",
      });
    }

    const out = engine.camRecommendFeeds<Json>(request);
    if (typeof out.error === "string") return err(out.error);
    const rec = out.recommendation as Json;
    const notes = (rec.notes as Json[] | undefined) ?? [];
    const body: Json = {
      ok: true,
      material: out.material,
      machine: out.machine,
      feed_mm_min: mm(rec.feed_mm_min),
      plunge_mm_min: mm(rec.plunge_mm_min),
      rpm: mm(rec.rpm),
      stepdown_mm: mm(rec.stepdown_mm),
      stepover_mm: mm(rec.stepover_mm),
      // A router with a manual speed dial ignores the S word entirely, so a
      // recommendation that only said "13500 rpm" would be unusable.
      dial: out.dial_spindle ? rec.dial : undefined,
      dial_spindle: out.dial_spindle,
      warnings: notes
        .filter((n) => n.level !== "info")
        .map((n) => `${String(n.level)}: ${String(n.text)}`),
      provisional: "These are published chiploads worked through, not measurements. Confirm on a test cut before committing stock.",
    };
    if (isFull(args)) body.recommendation = rec;
    return ok(body);
  });
}

// ---------------------------------------------------------------------------
// cam_job
// ---------------------------------------------------------------------------

export const camJobSchema = {
  type: "object" as const,
  required: ["stock", "tools", "operations"],
  properties: {
    ...sectionProperties,
    name: { type: "string" as const, description: "Job name, carried into the program's first comment." },
    stock: {
      type: "object" as const,
      description:
        "{ thickness, margin? | bbox?, spoilboard? } in mm. The stock top is Z0 and the underside is -thickness. Any cut past the underside needs a declared `spoilboard` at least that thick, or the job is refused.",
    },
    machine: {
      type: "object" as const,
      description:
        '{ name?, spindle: "dial" | "controlled", class?, max_feed?, max_accel?, travel: {min,max}?, work_offset? }. Give `travel` and `work_offset` and the envelope check tells you before the machine does.',
    },
    tools: {
      type: "array" as const,
      description:
        "Tool library: [{ number, kind?, diameter, flutes?, flute_length?, stickout?, centre_cutting?, holder? }]. Every operation names one by number.",
      items: { type: "object" as const },
    },
    operations: {
      type: "array" as const,
      description:
        'Operations, in any order — inside features are sequenced before the profile that frees the part. Each: { name?, tool, kind, depth, stepdown, feed, plunge, rpm, ... }. `kind` is face | pocket | contour_outside | contour_inside | drill | helical_bore. `contour` takes points OR a name from cam_outline ("outer", "hole:0"). Add `tabs` + `tab_width` + `tab_height` (or `tab_positions`) to hold the part, `bottom_allowance` for an onion skin (negative = break-through into a declared spoilboard), `placement { dx, dy, rotation_deg }` for stock that is not square.',
      items: { type: "object" as const },
    },
    options: {
      type: "object" as const,
      description:
        '{ part?, placement?, arc_fit?, tool_change?, post?, wcs?, end?, safe_z?, park_z?, verify? }. `part` (the outline the job is checked against) defaults to the sectioned part when `document_id` is given. `verify: false` turns the oracle off and is reported as a warning — the job then carries G-code nothing has checked.',
    },
    verify_policy: {
      type: "object" as const,
      description:
        'Per-check severity override, e.g. { "material_left": "warning" }. Downgrading a check is how a job that the oracle refuses gets run anyway, so it is recorded in the result.',
    },
    detail: {
      type: "string" as const,
      description:
        "'summary' (default) returns the verdict, the check summary, the op table and an artifact link to the G-code. 'full' also attaches the G-code text, the full verification report and the preview polyline.",
    },
  },
};

export function camJob(args: Json, engine: Engine): ToolResult {
  return guard(() => {
    const resolved = outlineFor(args, engine);
    const outline = resolved?.outline ?? null;

    // Contours may be named rather than spelled out; resolve before posting.
    const operations = ((args.operations as Json[] | undefined) ?? []).map((op, i) => {
      if (op.contour === undefined) return op;
      return {
        ...op,
        contour: resolveContour(
          op.contour,
          outline,
          `operation ${op.name ?? `#${i}`}.contour`,
        ),
      };
    });

    const options: Json = { ...((args.options as Json | undefined) ?? {}) };
    // The part the oracle replays against. Stating it is better than deriving
    // it from the operations, and the sectioned solid is the best statement
    // available: it is the part, not a description of the cuts.
    if (options.part === undefined && outline) {
      const region = (outline.regions as Json[] | undefined)?.[0];
      if (region) options.part = { outer: region.outer, holes: region.holes };
    }

    const out = engine.camJob<Json>({
      name: args.name,
      stock: args.stock,
      machine: args.machine,
      tools: args.tools,
      operations,
      options,
      verify_policy: args.verify_policy,
    });
    if (typeof out.error === "string" && out.blocked !== true) {
      return err(out.error);
    }

    const policy = (out.policy as Json | undefined) ?? {};
    const blocked = out.blocked === true;
    const full = isFull(args);
    const verification = out.verification as Json | undefined;
    const notes = ((out.notes as Json[] | undefined) ?? []).map(
      (n) => `${String(n.level)}: ${String(n.text)}`,
    );

    const opTable = ((out.op_ranges as Json[] | undefined) ?? [])
      .filter((r) => r.block === "operation")
      .map((r) => ({
        name: r.name,
        tool: r.tool,
        seconds: mm(r.seconds),
        moves: (num(r.end) ?? 0) - (num(r.start) ?? 0),
      }));

    const body: Json = {
      ok: !blocked,
      blocked,
      name: out.name,
      verified: policy.verified === true,
      blocked_by: policy.blocked_by ?? [],
      warnings: policy.warnings ?? [],
      checks: checkSummary(verification, full),
      tool_sequence: out.tool_sequence,
      operations: opTable,
      duration: {
        // The acceleration-aware number is the one to plan around; the naive
        // one is what a controller that never accelerated would take.
        accel_aware_s: mm((out.duration as Json | undefined)?.accel_aware_s),
        naive_s: mm((out.duration as Json | undefined)?.naive_s),
      },
      envelope: verification?.envelope
        ? {
            work_min: (((verification.envelope as Json).work_min as number[]) ?? []).map(
              (v) => Number(v.toFixed(3)),
            ),
            work_max: (((verification.envelope as Json).work_max as number[]) ?? []).map(
              (v) => Number(v.toFixed(3)),
            ),
          }
        : undefined,
      deepest_z: mm((verification?.depth as Json | undefined)?.deepest_z),
      tab_placement: out.tab_placement,
      tool_checks: ((out.tool_checks as Json[] | undefined) ?? []).map((c) => ({
        op: c.op,
        severity: c.severity,
        message: c.message,
      })),
      notes,
      safety: SAFETY,
    };
    // Deposited on a blocked job too: the claims are *why* it was refused,
    // and a ledger that only ever saw passing jobs is a record of nothing.
    const claims = deposit(
      args,
      out.claims,
      "cam.job",
      String(out.name ?? args.name ?? "job"),
    );
    if (claims) body.claims = claims;
    if (out.arc_fit) body.arc_fit = out.arc_fit;
    if (full) {
      body.moves = out.moves;
      body.op_ranges = out.op_ranges;
      if (out.fit) body.fit = out.fit;
    }

    if (blocked) {
      // The rule this tool exists to enforce. Nothing below writes a `gcode`
      // key, nothing above copied one in (the kernel does not emit one when it
      // refuses), and the refusal says what to change.
      body.gcode = null;
      body.why =
        out.error ??
        `Refused by the verification oracle: ${((policy.blocked_by as string[]) ?? []).join(", ")}. No G-code was produced, so there is nothing to export or send.`;
      body.next =
        "Fix the geometry the named check objects to, or — if you have judged the check wrong for this job — downgrade it explicitly with verify_policy, which is recorded in the result.";
      return ok(body);
    }

    const gcode = typeof out.gcode === "string" ? out.gcode : "";
    if (!gcode) {
      return err(
        "The job was not blocked but no G-code came back. This is a bug in the CAM surface — please report it with the request.",
      );
    }
    const filename = `${String(out.name ?? "job")
      .replace(/[^a-z0-9._-]+/gi, "-")
      .toLowerCase()}.nc`;
    const handle = storeArtifact([
      { name: filename, content: Buffer.from(gcode, "utf8") },
    ]);
    body.gcode = {
      filename,
      artifact_id: handle.artifact_id,
      artifact_url: handle.artifact_url,
      bytes: handle.bytes,
      lines: gcode.split("\n").length,
      sha256: handle.manifest[0]?.sha256,
      expires_at: handle.expires_at,
    };
    if (full) (body.gcode as Json).text = gcode;
    return ok(body);
  });
}

// ---------------------------------------------------------------------------
// cam_verify_gcode
// ---------------------------------------------------------------------------

export const camVerifyGcodeSchema = {
  type: "object" as const,
  required: ["gcode", "stock", "tool_diameter"],
  properties: {
    ...sectionProperties,
    gcode: { type: "string" as const, description: "The program text to replay." },
    part: {
      type: "string" as const,
      description:
        "Root part id or exact name to section for the part outline. Or give `part_outline` explicitly.",
    },
    part_outline: {
      type: "object" as const,
      description:
        "{ outer: [[x,y]…], holes: [[[x,y]…]…] } — the part the program is meant to make, when it is not a document part.",
    },
    stock: { type: "object" as const, description: "{ thickness, margin? | bbox?, spoilboard? }." },
    tool_diameter: { type: "number" as const, description: "Cutting diameter of the tool the program assumes, mm." },
    bottom_allowance: {
      type: "number" as const,
      description: "Onion skin left on the floor, mm. Negative means a deliberate break-through.",
    },
    tabs: {
      type: "array" as const,
      description: "Tabs the program is supposed to leave: [{ width, height }], one entry per tab.",
      items: { type: "object" as const },
    },
    machine: { type: "object" as const, description: "{ travel: {min,max}, work_offset } for the envelope check." },
    tolerance: { type: "number" as const, description: "Replay tolerance, mm (default 0.02)." },
    detail: { type: "string" as const, description: "'summary' (default) or 'full'." },
  },
};

export function camVerifyGcodeTool(args: Json, engine: Engine): ToolResult {
  return guard(() => {
    let part = args.part_outline as Json | undefined;
    if (!part) {
      const resolved = outlineFor(args, engine);
      if (!resolved) {
        return err(
          "Pass `document_id` (and `part` when the document holds several), or give `part_outline` — the program has to be replayed against something.",
        );
      }
      const region = (resolved.outline.regions as Json[] | undefined)?.[0];
      if (!region) return err("The section found no regions to verify against.");
      part = { outer: region.outer, holes: region.holes };
    }

    const out = engine.camVerifyGcode<Json>({
      gcode: args.gcode,
      part,
      stock: args.stock,
      tool_diameter: args.tool_diameter,
      bottom_allowance: args.bottom_allowance,
      tabs: args.tabs ?? [],
      machine: args.machine,
      tolerance: args.tolerance,
    });
    if (typeof out.error === "string") return err(out.error);

    const verification = out.verification as Json | undefined;
    const policy = (out.policy as Json | undefined) ?? {};
    return ok({
      ok: out.pass === true,
      pass: out.pass,
      blocked: out.blocked,
      blocked_by: policy.blocked_by ?? [],
      moves_replayed: verification?.moves,
      checks: checkSummary(verification, isFull(args)),
      verdict:
        out.blocked === true
          ? "This program does not make this part. Do not run it."
          : "This program makes this part, within the replay tolerance.",
      safety:
        "A 2.5D replay: it checks where the cutter goes against where the part is. It does not model deflection, chatter, or a machine that is not where it says it is.",
    });
  });
}

// ---------------------------------------------------------------------------
// cam_gear
// ---------------------------------------------------------------------------

export const camGearSchema = {
  type: "object" as const,
  required: ["gear"],
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "CAD session to file this gear's claims against. Optional — without it the claims come back in the result but nothing certifies them later. With it, build_receipt carries the over-pins prediction and record_measurement can close it.",
    },
    gear: {
      type: "object" as const,
      description:
        "{ module, teeth, pressure_angle?, profile_shift?, backlash_thinning?, internal?, face_width? }.",
    },
    cutter_diameter: {
      type: "number" as const,
      description: "End mill that cuts the tooth spaces, mm. Defaults to the module.",
    },
    pin_diameter: { type: "number" as const, description: "Pin to measure over, mm." },
    measured: {
      type: "number" as const,
      description:
        "An over-pins reading taken off a cut part, mm. Given with `pin_diameter`, the result carries the cutter offset that reading implies for the next part.",
    },
    span: { type: "boolean" as const, description: "Add a span-over-teeth measurement." },
    contours: {
      type: "boolean" as const,
      description: "Return tooth-space, full-profile and tool-centre point lists (default true; they are large — combine with detail).",
    },
    planetary: {
      type: "object" as const,
      description: "{ sun, planet, ring, planets } — check a train meshes and assembles.",
    },
    detail: {
      type: "string" as const,
      description: "'summary' (default) omits the contour point lists. 'full' attaches them.",
    },
  },
};

export function camGear(args: Json, engine: Engine): ToolResult {
  return guard(() => {
    const full = isFull(args);
    const out = engine.camGear<Json>({
      gear: args.gear,
      cutter_diameter: args.cutter_diameter,
      pin_diameter: args.pin_diameter,
      measured: args.measured,
      span: args.span,
      planetary: args.planetary,
      // The point lists are the expensive part of the answer; only build them
      // when the caller will actually read them.
      contours: args.contours === undefined ? full : args.contours,
    });
    if (typeof out.error === "string") return err(out.error);

    const report = out.report as Json;
    const contours = out.contours as Json | undefined;
    const body: Json = {
      ok: true,
      gear: out.gear,
      cutter_diameter: out.cutter_diameter,
      recommended_pin_diameter: mm(out.recommended_pin_diameter),
      over_pins_mm: mm((report?.over_pins as Json | undefined)?.dimension),
      report,
      contour_points: contours
        ? {
            tooth_space: (contours.tooth_space as unknown[] | undefined)?.length,
            full_profile: (contours.full_profile as unknown[] | undefined)?.length,
            tool_centre_path: (contours.tool_centre_path as unknown[] | undefined)?.length,
          }
        : undefined,
      compensation: out.compensation,
      planetary: out.planetary,
      provisional:
        "Exact involute geometry. Whether the teeth come out this size depends on the machine, the cutter and the material — measure over pins and pass `measured` to get the correction for the next part.",
    };
    if (full && contours) body.contours = contours;
    // The deposit carries the gear definition as well as the claims, which is
    // what lets a later measurement over pins come back as a cutter offset
    // rather than just "the teeth are fat".
    const claims = deposit(
      args,
      out.claims,
      "cam.gear",
      String(out.claim_subject ?? "gear"),
    );
    if (claims) {
      body.claims = {
        ...claims,
        subject: out.claim_subject,
        next: "record_measurement with claim_report_id, claim: \"gear.over_pins\" and the reading over pins closes this — and returns the compensation for the next part.",
      };
    }
    return ok(body);
  });
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

export const toolDefs: ToolDef[] = [
  {
    name: "cam_outline",
    pack: "cam",
    description:
      "Section a document part at a Z height into the closed contours a milling job cuts: the outer boundary, every hole, the circles among them (with fitted diameters), and a verdict on whether the part is constant-section at all — a 2.5D contour job only describes a part whose shape does not change with depth. Sections the part's RAW tessellation where there is a B-rep behind it, never the repaired export mesh, which can be torn by 0.4 mm where tangent fillets meet; a solid that does not close is refused with the gap positions rather than silently smoothed. Name the contours it returns (\"outer\", \"hole:0\") in cam_fit and cam_job instead of carrying the points.",
    inputSchema: camOutlineSchema,
    handler: (a, c) => camOutline(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "cam_fit",
    pack: "cam",
    description:
      "Ask whether a given cutter physically fits a contour: how many inside corners it cannot reach, how much metal that leaves and where the worst of it stands, plus the largest cutter that would still fit. Run it before cam_job — a cutter that does not fit does not fail, it quietly leaves wall standing in every corner, and the largest-tool number is the answer to 'so what should I use?'. Geometry only: it says nothing about deflection or chatter, so treat the verdict as necessary and not sufficient.",
    inputSchema: camFitSchema,
    handler: (a, c) => camFit(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "cam_recommend_feeds",
    pack: "cam",
    description:
      "Feeds, speeds, stepdown and stepover for a material and cutter on a machine of a given rigidity — including the dial position to set by hand, because on a trim router the S word does nothing and a recommendation that only said 'rpm' would be unusable. Pass `settings` instead to have numbers you already have checked against the table, or `list: true` for the materials. These are published chiploads worked through, not measurements of your machine: confirm on a test cut before committing stock.",
    inputSchema: camRecommendFeedsSchema,
    handler: (a, c) => camRecommendFeeds(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "cam_job",
    pack: "cam",
    description:
      "Post a whole milling job — several operations, several tools, one spindle start per tool — and replay it against the part it is meant to make before handing it over. Fails closed: if any error-severity check fails (gouging the part, cutting past the stock with nothing beneath, leaving the part loose, running outside the machine's travel) the result carries `blocked: true`, the checks that objected, and NO G-code anywhere — do not cut, and there is nothing to export by accident. On a pass the G-code comes back as an artifact link with the verification summary, the operation table and a cycle-time estimate; the time and the feeds are predictions until a part is measured.",
    inputSchema: camJobSchema,
    handler: (a, c) => camJob(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "cam_verify_gcode",
    pack: "cam",
    description:
      "Replay a G-code program you already have — off disk, out of another CAM package, edited by hand — against the part it is meant to make, and say whether it makes that part. Answers the question that cannot be asked of a file by reading it: does this cut the right side of every wall, stay out of the part, reach the right depth, leave the tabs it claims, and stay inside the machine. `blocked: true` means do not run it; a pass is a 2.5D geometric replay and does not model deflection, chatter, or a machine that is not where it says it is.",
    inputSchema: camVerifyGcodeSchema,
    handler: (a, c) => camVerifyGcodeTool(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "cam_gear",
    pack: "cam",
    description:
      "Exact involute gear geometry for cutting on a mill: the tooth-space contour the named cutter actually leaves, the reachability check (a cutter too fat for the space is refused rather than cutting a different tooth), the over-pins dimension to measure, contact ratio, and planetary-train meshing. Pass `measured` with the pin diameter you used and it returns the cutter offset that reading implies for the next part — the measurement loop is how a predicted tooth thickness becomes a known one.",
    inputSchema: camGearSchema,
    handler: (a, c) => camGear(a, c.engine),
    behavior: behavior({}),
  },
];
