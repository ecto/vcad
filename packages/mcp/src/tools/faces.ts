/**
 * Face-level B-rep queries: `inspect_faces` and `measure_outer_diameter`.
 *
 * The mesh-based tools next door (`inspect_cad`, `inspect_part`,
 * `describe_scene`, `measure`) are tessellation-bound and topology-blind —
 * they answer bbox / volume / centre-of-mass / min-distance and nothing
 * about the part's faces. That is not enough to reason mechanically: you
 * cannot find the mounting face, read a bore diameter, or get a shaft axis.
 * A bounding box will even lie about a diameter — a motor with an 80.0 mm
 * body reads ~102 mm across its bbox because of one radial connector boss.
 *
 * These two tools read the kernel's B-rep instead:
 *
 * - `inspect_faces` — faces with a stable id, surface type, area, bbox,
 *   centroid, and the analytic surface parameters; filterable by surface
 *   type / radius / area, paginated, and summarised by default so a vendor
 *   part with 800 faces doesn't dump 800 records.
 * - `measure_outer_diameter` — the largest coaxial cylinder about an axis,
 *   which is what "true outer diameter" actually means.
 *
 * Accuracy split, reported on every result:
 * - **analytic** (exact): surface type, cylinder radius/axis, plane
 *   normal/point, cone half-angle, sphere/torus radii.
 * - **tessellation-bound** (same caveat as `inspect_cad`): area, face bbox,
 *   centroid, axial extent.
 *
 * All lengths are mm, areas mm², angles degrees.
 */

import type {
  CoaxialGroup,
  DocumentFaceReport,
  Engine,
  FaceInfo,
  FaceReport,
} from "@vcad/engine";
import type { Document } from "@vcad/ir";

import { getSession } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";

/** Faces returned before the result switches to summary-only. */
const DEFAULT_LIMIT = 25;

/** Round to micron / µm² so payloads stay readable. */
const r6 = (n: number) => Math.round(n * 1e6) / 1e6;
const vec = (v: readonly number[]) => v.map(r6);

const ACCURACY_NOTE =
  "Surface parameters (radius, axis, normal, half-angle) are analytic and exact. " +
  "Area, bbox, centroid and axial extent come from the face's triangulation, so they are tessellation-bound.";

function err(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}

/** One part's face report plus the label the caller used to ask for it. */
interface ResolvedPart {
  name: string;
  node_id: string;
  report: FaceReport;
}

/**
 * Evaluate the document's B-rep faces, then narrow to the requested part.
 *
 * Fails closed with an explanatory message when the part is mesh-only (an
 * imported mesh, or a feature that dropped to tessellation) rather than
 * guessing face parameters from triangles.
 */
function resolveParts(
  doc: Document,
  engine: Engine,
  partRef: string | undefined,
): { parts: ResolvedPart[] } | { error: string } {
  let all: DocumentFaceReport;
  try {
    all = engine.documentFaces(doc);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  if (all.parts.length === 0) {
    return { error: "This document has no visible geometry to inspect." };
  }

  const wanted = partRef
    ? all.parts.filter((p) => p.node_id === partRef || p.name === partRef)
    : all.parts;

  if (wanted.length === 0) {
    return {
      error:
        `No part matched '${partRef}'. Available: ` +
        all.parts.map((p) => `${p.name} (${p.node_id})`).join(", "),
    };
  }

  const usable = wanted.filter((p) => p.brep && p.report);
  if (usable.length === 0) {
    const why = wanted.map((p) => `${p.name}: ${p.error}`).join("; ");
    return {
      error:
        `Face queries need B-rep topology, and no requested part has it — ${why}. ` +
        "Use `measure` / `inspect_part` for mesh-level answers on these parts.",
    };
  }

  return {
    parts: usable.map((p) => ({
      name: p.name,
      node_id: p.node_id,
      report: p.report as FaceReport,
    })),
  };
}

/** Compact per-face payload; rounds and drops nulls. */
function facePayload(f: FaceInfo) {
  const out: Record<string, unknown> = {
    face_id: f.id,
    stable_id: f.stable,
    surface_type: f.surface_type,
    area_mm2: r6(f.area_mm2),
    centroid_mm: vec(f.centroid_mm),
    bbox_mm: { min: vec(f.bbox_min_mm), max: vec(f.bbox_max_mm) },
  };
  if (f.inner_loops > 0) out.inner_loops = f.inner_loops;

  const s = f.surface;
  switch (s.kind) {
    case "plane":
      out.normal = vec(s.normal);
      out.point_on_plane_mm = vec(s.point);
      break;
    case "cylinder":
      out.radius_mm = r6(s.radius_mm);
      out.diameter_mm = r6(s.diameter_mm);
      out.axis = vec(s.axis);
      out.point_on_axis_mm = vec(s.axis_point);
      out.axial_range_mm = vec(s.axial_range_mm);
      out.axial_length_mm = r6(s.axial_length_mm);
      out.feature = s.convex ? "shaft_or_boss" : "bore";
      break;
    case "cone":
      out.apex_mm = vec(s.apex);
      out.axis = vec(s.axis);
      out.half_angle_deg = r6(s.half_angle_deg);
      break;
    case "sphere":
      out.center_mm = vec(s.center);
      out.radius_mm = r6(s.radius_mm);
      break;
    case "torus":
      out.center_mm = vec(s.center);
      out.axis = vec(s.axis);
      out.major_radius_mm = r6(s.major_radius_mm);
      out.minor_radius_mm = r6(s.minor_radius_mm);
      break;
    case "other":
      out.note = `${s.surface_type} surfaces have no closed-form parameters`;
      break;
  }
  return out;
}

function coaxialPayload(g: CoaxialGroup) {
  return {
    axis: vec(g.axis),
    point_on_axis_mm: vec(g.axis_point),
    outer_diameter_mm: r6(g.max_diameter_mm),
    outer_radius_mm: r6(g.max_radius_mm),
    inner_radius_mm: r6(g.min_radius_mm),
    radii_mm: g.radii_mm.map(r6),
    axial_range_mm: vec(g.axial_range_mm),
    lateral_area_mm2: r6(g.total_area_mm2),
    face_count: g.face_ids.length,
    face_ids: g.face_ids.slice(0, 12),
  };
}

// =============================================================================
// inspect_faces
// =============================================================================

interface FaceFilters {
  surfaceTypes?: string[];
  faceId?: string;
  minAreaMm2?: number;
  radiusMm?: number;
  radiusToleranceMm: number;
}

function applyFilters(faces: FaceInfo[], f: FaceFilters): FaceInfo[] {
  return faces.filter((face) => {
    if (f.faceId && face.id !== f.faceId) return false;
    if (f.surfaceTypes && !f.surfaceTypes.includes(face.surface_type)) {
      return false;
    }
    if (f.minAreaMm2 !== undefined && face.area_mm2 < f.minAreaMm2) return false;
    if (f.radiusMm !== undefined) {
      const s = face.surface;
      const r =
        s.kind === "cylinder" || s.kind === "sphere" ? s.radius_mm : undefined;
      if (r === undefined) return false;
      if (Math.abs(r - f.radiusMm) > f.radiusToleranceMm) return false;
    }
    return true;
  });
}

/** `inspect_faces` payload builder, shared with any in-app caller. */
export function inspectFacesResult(
  doc: Document,
  engine: Engine,
  args: {
    partRef?: string;
    filters: FaceFilters;
    limit: number;
    offset: number;
    summaryOnly: boolean;
  },
): Record<string, unknown> | { error: string } {
  const resolved = resolveParts(doc, engine, args.partRef);
  if ("error" in resolved) return resolved;

  const parts = resolved.parts.map((p) => {
    const filtered = applyFilters(p.report.faces, args.filters);
    // Faces arrive largest-area-first, which is the useful order: the
    // mounting face and the body OD come before the fastener chamfers.
    const page = args.summaryOnly
      ? []
      : filtered.slice(args.offset, args.offset + args.limit);

    const payload: Record<string, unknown> = {
      part: p.name,
      node_id: p.node_id,
      face_count: p.report.face_count,
      matched_faces: filtered.length,
      face_ids_stable: p.report.named,
      groups: p.report.groups.map((g) => ({
        surface_type: g.surface_type,
        ...(g.radius_mm !== undefined
          ? { radius_mm: r6(g.radius_mm), diameter_mm: r6(2 * g.radius_mm) }
          : {}),
        count: g.count,
        total_area_mm2: r6(g.total_area_mm2),
        example_face_ids: g.example_face_ids,
      })),
      coaxial_groups: p.report.coaxial_groups.slice(0, 10).map(coaxialPayload),
    };

    if (!args.summaryOnly) {
      payload.faces = page.map(facePayload);
      const shown = args.offset + page.length;
      if (shown < filtered.length) {
        payload.truncated = {
          showing: `${args.offset + 1}-${shown} of ${filtered.length}`,
          hint: "Pass `offset` for the next page, or narrow with `surface_type` / `radius_mm` / `min_area_mm2`.",
        };
      }
    }

    if (!p.report.named) {
      payload.id_note =
        "This part carries no topological names (imported or mesh-derived), so face ids are positional " +
        "(`face_<n>` in centroid order): stable for this geometry, but not across a rebuild that moves faces.";
    }
    return payload;
  });

  return { units: "mm", parts, note: ACCURACY_NOTE };
}

const inspectFacesSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document holding the part.",
    },
    part: {
      type: "string" as const,
      description:
        "Part id or name to inspect. Omit to inspect every visible part.",
    },
    surface_type: {
      type: "array" as const,
      items: {
        type: "string" as const,
        enum: ["plane", "cylinder", "cone", "sphere", "torus", "bspline", "bilinear"],
      },
      description:
        "Keep only faces of these surface types, e.g. ['cylinder'] for bores and shafts, ['plane'] for mounting faces.",
    },
    face_id: {
      type: "string" as const,
      description: "Return just this face (an id from an earlier call).",
    },
    radius_mm: {
      type: "number" as const,
      description:
        "Keep only cylindrical/spherical faces of this radius (within `radius_tolerance_mm`) — e.g. 2.1 to find every M4 clearance hole.",
    },
    radius_tolerance_mm: {
      type: "number" as const,
      description: "Tolerance for `radius_mm`. Default 0.01 mm.",
    },
    min_area_mm2: {
      type: "number" as const,
      description:
        "Drop faces smaller than this — the fast way past chamfers and fastener detail on a vendor part.",
    },
    summary_only: {
      type: "boolean" as const,
      description:
        "Return only the grouped tallies and coaxial groups, no per-face records. Best first call on an unfamiliar part.",
    },
    limit: {
      type: "number" as const,
      description: `Max faces to return per part. Default ${DEFAULT_LIMIT}.`,
    },
    offset: {
      type: "number" as const,
      description: "Faces to skip (pagination). Default 0.",
    },
  },
  required: ["document_id"],
} as const;

/** `inspect_faces` MCP handler. */
export function inspectFaces(args: Record<string, unknown>, engine: Engine) {
  const documentId = String(args.document_id ?? "");
  if (!documentId) return err("Pass `document_id` (the open CAD session).");

  let doc: Document;
  try {
    doc = getSession(documentId);
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }

  const surfaceTypes = Array.isArray(args.surface_type)
    ? args.surface_type.map((x) => String(x))
    : undefined;

  const payload = inspectFacesResult(doc, engine, {
    partRef: args.part === undefined ? undefined : String(args.part),
    filters: {
      surfaceTypes,
      faceId: args.face_id === undefined ? undefined : String(args.face_id),
      minAreaMm2:
        args.min_area_mm2 === undefined ? undefined : Number(args.min_area_mm2),
      radiusMm: args.radius_mm === undefined ? undefined : Number(args.radius_mm),
      radiusToleranceMm:
        args.radius_tolerance_mm === undefined
          ? 0.01
          : Number(args.radius_tolerance_mm),
    },
    limit: args.limit === undefined ? DEFAULT_LIMIT : Number(args.limit),
    offset: args.offset === undefined ? 0 : Number(args.offset),
    summaryOnly: args.summary_only === true,
  });

  if ("error" in payload) return err(String(payload.error));

  const body = { document_id: documentId, ...payload };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(body) }],
    structuredContent: { faces: body, document_id: documentId },
  };
}

// =============================================================================
// measure_outer_diameter
// =============================================================================

const outerDiameterSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document holding the part.",
    },
    part: {
      type: "string" as const,
      description: "Part id or name. Omit to measure every visible part.",
    },
    axis: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description:
        "Axis to measure about, e.g. [0,1,0]. Sign is ignored. Omit to use the part's dominant axis (the one carrying the most cylindrical area) — do not assume Z.",
    },
  },
  required: ["document_id"],
} as const;

/** `measure_outer_diameter` payload builder. */
export function outerDiameterResult(
  doc: Document,
  engine: Engine,
  partRef: string | undefined,
  axis: [number, number, number] | undefined,
): Record<string, unknown> | { error: string } {
  const resolved = resolveParts(doc, engine, partRef);
  if ("error" in resolved) return resolved;

  const parallel = (g: CoaxialGroup) => {
    if (!axis) return true;
    const [ax, ay, az] = axis;
    const len = Math.hypot(ax, ay, az);
    if (len < 1e-12) return false;
    const [bx, by, bz] = g.axis;
    const dot = (ax * bx + ay * by + az * bz) / len;
    return Math.abs(Math.abs(dot) - 1) < 1e-6;
  };

  const parts = resolved.parts.map((p) => {
    const candidates = p.report.coaxial_groups.filter(parallel);
    if (candidates.length === 0) {
      return {
        part: p.name,
        node_id: p.node_id,
        error: axis
          ? `No cylindrical faces on this part share the axis [${axis.join(", ")}]. ` +
            `Axes present: ${p.report.coaxial_groups
              .map((g) => `[${g.axis.map(r6).join(", ")}]`)
              .join(", ") || "none (no cylindrical faces)"}.`
          : "This part has no cylindrical faces, so it has no outer diameter.",
      };
    }
    // With an axis given, the biggest cylinder on it is the OD. Without one,
    // the dominant axis is the group carrying the most lateral area — the
    // groups arrive in that order.
    const chosen = axis
      ? candidates.reduce((a, b) => (b.max_radius_mm > a.max_radius_mm ? b : a))
      : candidates[0];

    const bboxSpan = (() => {
      const lo = [0, 1, 2].map((i) =>
        Math.min(...p.report.faces.map((f) => f.bbox_min_mm[i])),
      );
      const hi = [0, 1, 2].map((i) =>
        Math.max(...p.report.faces.map((f) => f.bbox_max_mm[i])),
      );
      return [0, 1, 2].map((i) => r6(hi[i] - lo[i]));
    })();

    return {
      part: p.name,
      node_id: p.node_id,
      axis_selection: axis ? "requested" : "dominant",
      ...coaxialPayload(chosen),
      other_axes: p.report.coaxial_groups
        .filter((g) => g !== chosen)
        .slice(0, 5)
        .map((g) => ({
          axis: vec(g.axis),
          outer_diameter_mm: r6(g.max_diameter_mm),
        })),
      bbox_size_mm: bboxSpan,
    };
  });

  return {
    units: "mm",
    parts,
    note:
      "Diameters are analytic (exact). `bbox_size_mm` is given for contrast only — a bounding box " +
      "overstates diameter whenever a boss, connector or flange sticks out, which is why this tool exists.",
  };
}

/** `measure_outer_diameter` MCP handler. */
export function measureOuterDiameter(
  args: Record<string, unknown>,
  engine: Engine,
) {
  const documentId = String(args.document_id ?? "");
  if (!documentId) return err("Pass `document_id` (the open CAD session).");

  let doc: Document;
  try {
    doc = getSession(documentId);
  } catch (e) {
    return err(e instanceof Error ? e.message : String(e));
  }

  let axis: [number, number, number] | undefined;
  if (args.axis !== undefined) {
    if (!Array.isArray(args.axis) || args.axis.length !== 3) {
      return err("Pass `axis` as three numbers, e.g. [0, 1, 0].");
    }
    const a = args.axis.map((x) => Number(x));
    if (a.some((x) => !Number.isFinite(x))) {
      return err("`axis` components must be finite numbers.");
    }
    if (Math.hypot(a[0], a[1], a[2]) < 1e-12) {
      return err("`axis` must not be the zero vector.");
    }
    axis = [a[0], a[1], a[2]];
  }

  const payload = outerDiameterResult(
    doc,
    engine,
    args.part === undefined ? undefined : String(args.part),
    axis,
  );
  if ("error" in payload) return err(String(payload.error));

  const body = { document_id: documentId, ...payload };
  return {
    content: [{ type: "text" as const, text: JSON.stringify(body) }],
    structuredContent: { outer_diameter: body, document_id: documentId },
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "inspect_faces",
    pack: null,
    description:
      "List a part's B-rep faces: stable face id, surface type, area, bbox, centroid, plus the exact surface parameters — a cylinder's radius/diameter, axis direction, point on axis and axial extent (and whether it's a bore or a shaft); a plane's outward normal and a point on it. This is the tool for mechanical reasoning an agent can't do from a bounding box: find the mounting face, read a bore diameter, get a shaft axis. Filter with `surface_type` / `radius_mm` / `min_area_mm2`, page with `limit`/`offset`, or start with `summary_only` to get grouped tallies ('17 cylindrical faces at radius 2.1 mm') instead of hundreds of records. Radii, axes and normals are analytic; areas, bboxes and centroids are tessellation-bound. Complements `inspect_cad` and `measure`, which see only the triangle mesh. Requires B-rep geometry — mesh-only parts are refused by name, not guessed at.",
    inputSchema: inspectFacesSchema,
    handler: (a, c) => inspectFaces(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "measure_outer_diameter",
    pack: null,
    description:
      "Measure a part's true outer diameter: the largest cylinder coaxial with a given axis (omit `axis` to use the part's dominant axis — do not assume Z). This is what 'OD' means on a real part; a bounding box overstates it whenever a connector boss, flange or mounting ear sticks out, so the result reports the bbox size alongside for contrast. Also reports the smallest radius on that axis (the innermost bore), every distinct radius present, and the axial extent. Diameters are analytic, so exact regardless of tessellation.",
    inputSchema: outerDiameterSchema,
    handler: (a, c) => measureOuterDiameter(a, c.engine),
    behavior: behavior({}),
  },
];
