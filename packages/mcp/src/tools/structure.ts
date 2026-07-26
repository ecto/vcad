/**
 * analyze_structure tool — the `vcad-kernel-fea` crate over MCP.
 *
 * Static structural FEA on a part's real evaluated geometry: the
 * tessellated interior is filled with linear tetrahedra at two (or more)
 * lattice refinements and solved (linear elasticity, matrix-free PCG).
 * The result is fail-closed: QoIs must agree across refinement levels
 * within stated tolerances or the verdict is `Unverifiable` and no
 * predicted claim is emitted — a single-resolution FE number is an
 * anecdote, and this tool refuses to sell one. Converged results carry
 * `vcad.fea-claims/1` plus unified-receipt claims with basis
 * `"predicted"`, which roll up Provisional (never Pass) until the part
 * is load-tested.
 *
 * `predict_physics` remains the fast voxel-hex steering loop; this tool
 * is the "will this bracket break?" answer with the convergence audit
 * attached (safety factor included when a yield strength is given).
 *
 * `beam_check` is the third route, and the one to reach for on thin-walled
 * geometry — sheet metal, tube frames, plate. A lattice needs several cells
 * through the thinnest load-bearing section, and a 2 mm wall on a 312 mm
 * member wants a 0.33 mm pitch (~950 cells along the longest axis), far past
 * any affordable cap. So `analyze_structure` now *measures* the part and, when
 * the pitch cannot resolve the section, fails closed with the cell arithmetic
 * and this route already named. For a prismatic member the closed form is not
 * a consolation prize: exact section properties plus Bredt thin-wall torsion
 * beat a staircased lattice outright, and carry the same predicted-basis
 * receipt.
 */

import { getSession } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { resolvePartMesh } from "./topopt.js";

const regionSchema = {
  type: "object" as const,
  required: ["min", "max"],
  properties: {
    min: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Minimum corner [x, y, z] in mm (world frame, Z-up).",
    },
    max: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Maximum corner [x, y, z] in mm.",
    },
  },
};

interface StructureArgs {
  document_id: string;
  part: string;
  loads: Array<{ region: { min: number[]; max: number[] }; force: number[] }>;
  supports: Array<{ region: { min: number[]; max: number[] }; fix?: boolean[] }>;
  youngs_modulus_mpa?: number;
  poisson?: number;
  yield_strength_mpa?: number;
  resolution?: number;
  levels?: number;
  displacement_tol?: number;
  stress_tol?: number;
}

function textResult(payload: unknown) {
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(payload, null, 2),
      },
    ],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "analyze_structure",
    pack: null,
    description:
      "Static structural FEA on a part's real geometry with fail-closed mesh-convergence " +
      "gating: linear-tet fill of the tessellated interior at 2+ refinement levels, " +
      "linear-elastic solve (PCG), returning max von Mises stress, max displacement, " +
      "discretization-error estimates, and (with yield_strength_mpa) a safety factor — " +
      "plus `vcad.fea-claims/1` and unified-receipt claims. If QoIs disagree across " +
      "levels the verdict is Unverifiable and NO stress/displacement claim is emitted " +
      "(raise resolution, or fillet the singular corner it points at). Claims carry " +
      'basis "predicted" and roll up Provisional until hardware is load-tested. Scope: ' +
      "small-displacement linear elasticity, one isotropic material; no plasticity, " +
      "buckling, contact, or dynamic loads; boundary is staircase-approximated at the " +
      "lattice pitch. THIN-WALLED PARTS (sheet metal, tube frame, plate) are outside this " +
      "tool: it measures the thinnest load-bearing section and fails closed with the cell " +
      "arithmetic and a pointer to beam_check, which is the more accurate answer for a " +
      "prismatic member anyway. Use predict_physics for the fast coarse steering loop; use " +
      "this for the audited answer on chunky geometry.",
    inputSchema: {
      type: "object" as const,
      required: ["document_id", "part", "loads", "supports"],
      properties: {
        document_id: {
          type: "string" as const,
          description: "Session document containing the part.",
        },
        part: {
          type: "string" as const,
          description: "Root part id or exact name; its evaluated mesh is analyzed.",
        },
        loads: {
          type: "array" as const,
          minItems: 1,
          description:
            "Loads: total force (N) split over the mesh nodes inside each world-frame " +
            "box region. A zero-thickness box on a face selects that face's nodes. " +
            "Fail-closed: a region that selects no node is an error.",
          items: {
            type: "object" as const,
            required: ["region", "force"],
            properties: {
              region: regionSchema,
              force: {
                type: "array" as const,
                items: { type: "number" as const },
                minItems: 3,
                maxItems: 3,
                description: "Total force [fx, fy, fz] in N.",
              },
            },
          },
        },
        supports: {
          type: "array" as const,
          minItems: 1,
          description: "Fixed (anchored) regions.",
          items: {
            type: "object" as const,
            required: ["region"],
            properties: {
              region: regionSchema,
              fix: {
                type: "array" as const,
                items: { type: "boolean" as const },
                minItems: 3,
                maxItems: 3,
                description: "Which translations are fixed [x, y, z]; default all true.",
              },
            },
          },
        },
        youngs_modulus_mpa: {
          type: "number" as const,
          description:
            "Young's modulus, MPa. Default 69000 (6061 aluminum); steel ~200000, PLA ~2300.",
        },
        poisson: {
          type: "number" as const,
          description: "Poisson's ratio in [0, 0.5). Default 0.33.",
        },
        yield_strength_mpa: {
          type: "number" as const,
          description:
            "Material yield strength, MPa. When given, safety_factor = yield / max von " +
            "Mises is computed and claimed (converged studies only).",
        },
        resolution: {
          type: "number" as const,
          description:
            "Coarse-level lattice cells along the longest bbox axis (default 24). The " +
            "finest level is resolution * 2^(levels-1), capped at 160 for this tier. " +
            "Keep >= ~6 cells through the thinnest load-bearing section — the tool measures " +
            "the part and refuses below ~4, with the cell arithmetic attached. Raising this " +
            "is NOT the lever for a thin wall (a 2 mm wall on a 300 mm member needs ~950 " +
            "cells): use beam_check.",
        },
        levels: {
          type: "number" as const,
          description: "Refinement levels (default 2, each 2x the previous).",
        },
        displacement_tol: {
          type: "number" as const,
          description:
            "Convergence gate on max displacement change between the two finest levels " +
            "(default 0.05 = 5%).",
        },
        stress_tol: {
          type: "number" as const,
          description:
            "Convergence gate on max von Mises change (default 0.15 = 15%; pointwise " +
            "stress converges slower, and diverges at sharp re-entrant corners).",
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as unknown as StructureArgs;
      const doc = getSession(String(a.document_id));
      const resolved = resolvePartMesh(doc, ctx.engine, String(a.part));
      const spec = {
        resolution: a.resolution ?? 24,
        youngs_modulus_mpa: a.youngs_modulus_mpa ?? 69_000,
        poisson: a.poisson ?? 0.33,
        yield_strength_mpa: a.yield_strength_mpa ?? null,
        loads: a.loads,
        supports: a.supports.map((s) => ({
          region: s.region,
          fix: s.fix ?? [true, true, true],
        })),
      };
      const options: Record<string, unknown> = {};
      if (a.levels !== undefined) options.levels = a.levels;
      if (a.displacement_tol !== undefined) options.displacement_tol = a.displacement_tol;
      if (a.stress_tol !== undefined) options.stress_tol = a.stress_tol;
      const started = performance.now();
      const out = ctx.engine.feaAnalyzeMesh(
        JSON.stringify(spec),
        JSON.stringify(options),
        resolved.mesh.positions,
        resolved.mesh.indices,
      );
      const solveMs = Math.round(performance.now() - started);
      return textResult({
        document_id: a.document_id,
        part: resolved.name ?? a.part,
        solve_ms: solveMs,
        ...(out as Record<string, unknown>),
      });
    },
    behavior: behavior({}),
  },
  {
    name: "beam_check",
    pack: null,
    description:
      "Closed-form structural check of a PRISMATIC member — the audited answer for " +
      "thin-walled geometry, where the lattice in analyze_structure cannot put enough cells " +
      "through the wall at any affordable resolution. Give it a profile (rect, rect_tube, " +
      "round, round_tube, i_beam), a span, an end condition, and a load case; get section " +
      "properties (A, I_y, I_z, J, section moduli, torsional stiffness), stresses (bending, " +
      "axial, torsional and transverse shear, von Mises), deflection with the Timoshenko " +
      "shear term, twist, the Euler buckling load under compression, and a safety factor — " +
      "plus `vcad.fea-claims/1` and unified-receipt claims under `structure.beam.*`. " +
      "Exact integrals where they exist (round, round tube, rectangle bending), the " +
      "convergent Saint-Venant series for solid-rectangle torsion, Bredt closed thin-wall " +
      "theory for tube torsion — every approximation is stated in the returned notes. For a " +
      "constant cross-section this is not a fallback: it beats a staircased lattice on " +
      "accuracy and costs microseconds. Fail-closed: too stubby for beam theory (L/depth < " +
      "5), too thick-walled for Bredt, deflecting past a tenth of the span, torque on an " +
      "open section, or buckling before it yields — any of those makes the verdict " +
      "Unverifiable and NO QoI is claimed, with the reason naming the route forward. Claims " +
      'carry basis "predicted" and roll up Provisional until hardware is load-tested. Scope: ' +
      "one uniform section over the whole span, idealized ends, no stress concentrations " +
      "(holes, welds, corner radii), no local wall buckling, no fatigue. Needs no document — " +
      "it is geometry-by-description, so it works before the part exists.",
    inputSchema: {
      type: "object" as const,
      required: ["profile", "length_mm", "end_condition"],
      properties: {
        profile: {
          type: "object" as const,
          required: ["type"],
          description:
            "Cross-section. The member runs along X and the section lives in (Y, Z): " +
            'width_mm is the Y extent, height_mm the Z extent. One of: {"type":"rect",' +
            'width_mm,height_mm}, {"type":"rect_tube",width_mm,height_mm,wall_mm}, ' +
            '{"type":"round",diameter_mm}, {"type":"round_tube",diameter_mm,wall_mm}, ' +
            '{"type":"i_beam",width_mm,height_mm,flange_mm,web_mm}. Tube dimensions are ' +
            "OUTSIDE dimensions.",
          properties: {
            type: {
              type: "string" as const,
              enum: ["rect", "rect_tube", "round", "round_tube", "i_beam"],
            },
            width_mm: { type: "number" as const, description: "Y extent, mm." },
            height_mm: { type: "number" as const, description: "Z extent, mm." },
            wall_mm: { type: "number" as const, description: "Wall thickness, mm." },
            diameter_mm: { type: "number" as const, description: "Outside diameter, mm." },
            flange_mm: { type: "number" as const, description: "Flange thickness, mm." },
            web_mm: { type: "number" as const, description: "Web thickness, mm." },
          },
        },
        length_mm: {
          type: "number" as const,
          description: "Free span between supports, mm.",
        },
        end_condition: {
          type: "string" as const,
          enum: [
            "cantilever_tip",
            "cantilever_uniform",
            "simple_center",
            "simple_uniform",
            "fixed_fixed_center",
            "fixed_fixed_uniform",
          ],
          description:
            "Support and load arrangement. `*_uniform` spreads the transverse force over the " +
            "span; `*_center`/`*_tip` concentrates it. Also sets the Euler effective-length " +
            "factor (cantilever K=2, simple K=1, fixed-fixed K=0.5).",
        },
        transverse_force_n: {
          type: "number" as const,
          description: "Total transverse force, N (magnitude). Default 0.",
        },
        bend_axis: {
          type: "string" as const,
          enum: ["y", "z"],
          description:
            "Which principal axis to bend about: `y` uses I_y and deflects along Z (default), " +
            "`z` uses I_z and deflects along Y. Matters for any non-square section.",
        },
        torque_nmm: {
          type: "number" as const,
          description:
            "Torque about the member axis, N·mm (1 N·m = 1000 N·mm). Default 0. This is the " +
            "case closed-form theory answers best and the lattice answers worst.",
        },
        axial_force_n: {
          type: "number" as const,
          description:
            "Axial force, N — positive tension, negative compression. Compression is also " +
            "checked against Euler buckling, and a member that buckles is reported " +
            "Unverifiable rather than 'safe'. Default 0.",
        },
        youngs_modulus_mpa: {
          type: "number" as const,
          description:
            "Young's modulus, MPa. Default 69000 (6061 aluminum); steel ~200000, PLA ~2300.",
        },
        poisson: {
          type: "number" as const,
          description: "Poisson's ratio in [0, 0.5). Default 0.33; sets G = E/(2(1+nu)).",
        },
        yield_strength_mpa: {
          type: "number" as const,
          description:
            "Yield strength, MPa. When given, safety_factor = yield / von Mises is computed " +
            "and claimed (applicable checks only).",
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as unknown as Record<string, unknown>;
      const started = performance.now();
      const out = ctx.engine.feaCheckBeam(JSON.stringify(a));
      return textResult({
        solve_ms: Math.round(performance.now() - started),
        ...(out as Record<string, unknown>),
      });
    },
    behavior: behavior({}),
  },
];
