/**
 * `check_enclosure_fit` — cross-domain PCB ↔ enclosure verification.
 *
 * The differentiator no EDA tool can match: vcad holds a real BRep CAD kernel
 * *and* a PCB engine in one session store, so it can co-verify a board against
 * the physical case it ships in. This tool takes a board session and a CAD
 * session holding the enclosure solid, extracts the case cavity (floor, walls,
 * standoffs, wall cutouts) from its mesh, and runs the four
 * {@link checkEnclosureFit} checks: board fits with clearance, components clear
 * the lid, mounting holes land on standoffs, connectors line up with cutouts.
 *
 * The verdict is also surfaced through `build_receipt` (attach an
 * `enclosure_document_id`) so a board's durable proof can include "and it fits
 * its case."
 */

import type { Document, Pcb } from "@vcad/ir";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import {
  checkEnclosureFit as computeEnclosureFit,
  componentMeshes,
  componentExtentsFromMeshes,
  connectorsFromPcb,
  deriveBoardFromCavity,
  extractEnclosureFeatures,
  mountingHolesFromPcb,
  type BoardPlacement,
  type ComponentExtent,
  type Engine,
  type EnclosureFitReport,
  type TriangleMesh,
} from "@vcad/engine";
import { getSession } from "./session.js";

/** PCB data from a document — PcbBoard nodes first, legacy `doc.pcb` fallback. */
function getDocPcb(doc: Document): Pcb | null {
  const nodeIds = getPcbNodeIds(doc);
  if (nodeIds.length > 0) return getNodePcb(doc, nodeIds[0]!);
  return (doc as Document & { pcb?: Pcb }).pcb ?? null;
}

const vec3Schema = {
  type: "object" as const,
  properties: {
    x: { type: "number" as const },
    y: { type: "number" as const },
    z: { type: "number" as const },
  },
  required: ["x", "y", "z"],
};

export const checkEnclosureFitSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Board session id (from create_schematic / place_components) holding the PCB to verify.",
    },
    enclosure_document_id: {
      type: "string" as const,
      description:
        "CAD session id (from open_document / create_cad_loon) holding the enclosure solid the board lives in.",
    },
    enclosure_part_id: {
      type: "string" as const,
      description: "Root id of the enclosure part when the CAD session has more than one solid.",
    },
    clearance: {
      type: "number" as const,
      description: "All-round clearance the board needs from the cavity walls and lid (mm). Default 0.5.",
    },
    standoff_height: {
      type: "number" as const,
      description: "Board lift above the cavity floor when the case has no detectable standoffs (mm). Default 0.",
    },
    board_offset: {
      ...vec3Schema,
      description:
        "Where the board's local origin sits in the enclosure-world frame. Omit to auto-fit (center the board on the standoffs).",
    },
    board_rotation_deg: {
      type: "number" as const,
      description: "Board rotation about Z in the enclosure frame (degrees). Default 0.",
    },
    hole_tolerance: {
      type: "number" as const,
      description: "Mounting-hole to standoff alignment tolerance (mm). Default 0.6.",
    },
    derive: {
      type: "boolean" as const,
      description:
        "Also return a board outline + mounting holes auto-derived from the cavity (the co-design starting point).",
    },
  },
  required: ["document_id", "enclosure_document_id"],
} as const;

/** Evaluate a CAD session and return the chosen solid's mesh (skips PcbBoard). */
function enclosureMesh(
  enclosureDoc: Document,
  engine: Engine,
  partId?: string,
): { mesh: TriangleMesh; rootId: number; name?: string } {
  const scene = engine.evaluate(enclosureDoc);
  const visibleRoots = enclosureDoc.roots.filter((e) => e.visible !== false);
  const candidates: Array<{ rootId: number; name?: string; mesh: TriangleMesh }> = [];
  for (let i = 0; i < scene.parts.length && i < visibleRoots.length; i++) {
    const rootId = visibleRoots[i].root;
    const node = enclosureDoc.nodes[String(rootId)];
    const opType = (node?.op as { type?: string } | undefined)?.type;
    if (opType === "PcbBoard") continue;
    const mesh = scene.parts[i].mesh;
    if (!mesh || mesh.positions.length === 0) continue;
    candidates.push({ rootId, name: node?.name ?? undefined, mesh });
  }
  if (candidates.length === 0) {
    throw new Error("Enclosure document has no solid parts (PcbBoard parts are excluded)");
  }
  if (partId) {
    const found = candidates.find((c) => String(c.rootId) === partId);
    if (!found) {
      throw new Error(
        `No enclosure part with id "${partId}". Available: ${candidates
          .map((c) => `${c.rootId}${c.name ? ` (${c.name})` : ""}`)
          .join(", ")}`,
      );
    }
    return found;
  }
  if (candidates.length > 1) {
    // Largest mesh wins — the case body dwarfs an incidental lid/fastener.
    candidates.sort((a, b) => b.mesh.positions.length - a.mesh.positions.length);
  }
  return candidates[0];
}

/** Result of the shared computation, reused by the tool and build_receipt. */
export interface EnclosureFitResult {
  report?: EnclosureFitReport;
  derived?: {
    outline: ReturnType<typeof deriveBoardFromCavity>["outline"];
    mountingHoles: ReturnType<typeof deriveBoardFromCavity>["mountingHoles"];
    placement: BoardPlacement;
  };
  cavity?: ReturnType<typeof extractEnclosureFeatures>["cavity"];
  standoffs_detected?: number;
  openings_detected?: number;
  error?: string;
}

/**
 * Core: given a board PCB and an enclosure CAD session, extract the cavity and
 * run the cross-domain checks. Pure of MCP plumbing so `check_enclosure_fit`
 * and `build_receipt` share one implementation.
 */
export async function computeEnclosureFitForBoard(
  pcb: Pcb,
  enclosureDoc: Document,
  engine: Engine,
  opts: {
    enclosurePartId?: string;
    clearance?: number;
    standoffHeight?: number;
    holeTolerance?: number;
    placement?: BoardPlacement;
    derive?: boolean;
  } = {},
): Promise<EnclosureFitResult> {
  let chosen: { mesh: TriangleMesh; rootId: number; name?: string };
  try {
    chosen = enclosureMesh(enclosureDoc, engine, opts.enclosurePartId);
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }

  const features = extractEnclosureFeatures(chosen.mesh.positions, chosen.mesh.indices);
  if (!features.cavity) {
    return {
      error:
        "No interior cavity found in the enclosure solid — it reads as a solid block. " +
        "check_enclosure_fit needs an open or closed case with a pocket the board sits in.",
    };
  }

  // Component Z extents from the kernel's 3D component bodies (best-effort: an
  // empty list degrades the lid check to a skip, never an error).
  let componentExtents: ComponentExtent[] = [];
  try {
    const meshes = await componentMeshes(pcb);
    if (meshes.length > 0) componentExtents = componentExtentsFromMeshes(meshes, pcb);
  } catch {
    /* WASM unavailable — lid clearance check will report skip */
  }

  const mountingHoles = mountingHolesFromPcb(pcb);
  const connectors = connectorsFromPcb(pcb, pcb.outline);

  const report = computeEnclosureFit({
    outline: pcb.outline,
    cavity: features.cavity,
    standoffs: features.standoffs,
    openings: features.openings,
    mountingHoles,
    connectors,
    componentExtents,
    placement: opts.placement,
    clearance: opts.clearance,
    standoffHeight: opts.standoffHeight,
    holeTolerance: opts.holeTolerance,
  });

  const result: EnclosureFitResult = {
    report,
    cavity: features.cavity,
    standoffs_detected: features.standoffs.length,
    openings_detected: features.openings.length,
  };
  if (opts.derive) {
    result.derived = deriveBoardFromCavity(features.cavity, features.standoffs, {
      clearance: opts.clearance,
      thickness: pcb.outline.thickness,
      standoffHeight: opts.standoffHeight,
    });
  }
  return result;
}

/** Read a board placement from tool args (offset + rotation), if present. */
function placementFromArgs(args: Record<string, unknown>): BoardPlacement | undefined {
  const off = args.board_offset as { x?: number; y?: number; z?: number } | undefined;
  if (!off || typeof off.x !== "number" || typeof off.y !== "number" || typeof off.z !== "number") {
    return undefined;
  }
  return {
    offset: { x: off.x, y: off.y, z: off.z },
    rotationDeg: typeof args.board_rotation_deg === "number" ? args.board_rotation_deg : 0,
  };
}

/** `check_enclosure_fit` MCP handler. */
export async function checkEnclosureFit(args: Record<string, unknown>, engine: Engine) {
  const boardId = String(args.document_id ?? "");
  const enclosureId = String(args.enclosure_document_id ?? "");
  if (!boardId) return err("Pass `document_id` (the board session).");
  if (!enclosureId) return err("Pass `enclosure_document_id` (the CAD session with the enclosure solid).");

  const boardDoc = getSession(boardId);
  const pcb = getDocPcb(boardDoc);
  if (!pcb) return err("Board document has no PCB — run place_components first.");
  const enclosureDoc = getSession(enclosureId);

  const res = await computeEnclosureFitForBoard(pcb, enclosureDoc, engine, {
    enclosurePartId: args.enclosure_part_id ? String(args.enclosure_part_id) : undefined,
    clearance: typeof args.clearance === "number" ? args.clearance : undefined,
    standoffHeight: typeof args.standoff_height === "number" ? args.standoff_height : undefined,
    holeTolerance: typeof args.hole_tolerance === "number" ? args.hole_tolerance : undefined,
    placement: placementFromArgs(args),
    derive: args.derive === true,
  });

  if (res.error || !res.report) return err(res.error ?? "Enclosure fit could not be computed.");

  const payload = {
    success: true,
    document_id: boardId,
    enclosure_document_id: enclosureId,
    ok: res.report.ok,
    verified: res.report.verified,
    summary: res.report.summary,
    clearance_mm: res.report.clearance,
    placement: res.report.placement,
    cavity: res.cavity,
    standoffs_detected: res.standoffs_detected,
    openings_detected: res.openings_detected,
    checks: res.report.checks,
    ...(res.derived ? { derived_board: res.derived } : {}),
  };

  return {
    content: [{ type: "text" as const, text: JSON.stringify(payload) }],
    structuredContent: {
      enclosure_fit: res.report,
      ...(res.derived ? { derived_board: res.derived } : {}),
      document_id: boardId,
      enclosure_document_id: enclosureId,
    },
  };
}

function err(text: string) {
  return {
    content: [{ type: "text" as const, text: `Error: ${text}` }],
    isError: true as const,
  };
}
