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
 */

import { getSession } from "./session.js";
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
      "lattice pitch. Use predict_physics for the fast coarse steering loop; use this " +
      "for the audited answer.",
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
            "Keep >= ~6 cells through the thinnest load-bearing section.",
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
];
