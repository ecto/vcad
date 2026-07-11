/**
 * predict_physics tool — two-tier static structural analysis.
 *
 * The fast inner loop of physics-validated generation: voxel FEA over a
 * part's volume (or a box) under given loads and supports. `fidelity`
 * picks the tier — `predict` answers in ~100 ms at coarse resolution and
 * stamps every claim `basis: predicted`; `verify` re-runs the SAME solver
 * at fine resolution and stamps `basis: verified`. A receipt whose passing
 * claims rest on predictions rolls up `provisional`, never `pass`
 * (crates/vcad-receipt) — predictions steer, verification certifies.
 */

import type { Engine, StaticAnalysisSpec, StaticAnalysisResult } from "@vcad/engine";
import type { DesignReceipt, ReceiptClaim, OracleRef } from "@vcad/ir";
import { getSession } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { resolvePartMesh } from "./topopt.js";
import { RECEIPT_SCHEMA, summarize, unverifiableClaim } from "../receipt-unified.js";

const PHYSICS_DOMAIN = "mechanical";
const ORACLE: OracleRef = { id: "vcad-kernel-topopt/static-fea", version: "0.9.4" };

/** Resolution per fidelity tier. Same solver; the grid is the only dial. */
const TIER_RESOLUTION = { predict: 32, verify: 72 } as const;

const regionSchema = {
  type: "object" as const,
  required: ["min", "max"],
  properties: {
    min: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 3,
      maxItems: 3,
      description: "Minimum corner [x, y, z] in mm.",
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

export const predictPhysicsSchema = {
  type: "object" as const,
  required: ["loads", "supports"],
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session document. Required with `part`.",
    },
    part: {
      type: "string" as const,
      description:
        "Part id or name to analyze (its evaluated volume is voxelized). " +
        "Mutually exclusive with `domain_box`.",
    },
    domain_box: {
      ...regionSchema,
      description:
        "Analyze a solid axis-aligned box (mm, world frame, Z-up) instead " +
        "of a part. Mutually exclusive with `part`.",
    },
    loads: {
      type: "array" as const,
      minItems: 1,
      description:
        "Loads: total force vectors (N) distributed over the grid nodes in " +
        "each world-frame box region. A zero-thickness box selects the " +
        "nearest plane of nodes.",
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
            description:
              "Which translations are fixed [x, y, z]; default all true.",
          },
        },
      },
    },
    fidelity: {
      type: "string" as const,
      enum: ["predict", "verify"],
      description:
        "`predict` (default): coarse fast solve, claims stamped " +
        "basis=predicted — good enough to steer a design. `verify`: fine " +
        "solve with the same oracle, claims stamped basis=verified — good " +
        "enough to certify. A receipt passing only on predicted claims " +
        "reads `provisional`, never `pass`.",
    },
    resolution: {
      type: "number" as const,
      description:
        "Override voxels along the longest axis (predict=32, verify=72 by " +
        "default). Below ~4 elements through the thinnest section, bending " +
        "results are unreliable.",
    },
    youngs_modulus_mpa: {
      type: "number" as const,
      description: "Young's modulus in MPa. Default 69000 (6061 aluminum).",
    },
    poisson: {
      type: "number" as const,
      description: "Poisson's ratio. Default 0.33.",
    },
    max_displacement_mm: {
      type: "number" as const,
      description:
        "Optional limit: assert max displacement ≤ this (claim " +
        "physics.static.displacement).",
    },
    max_von_mises_mpa: {
      type: "number" as const,
      description:
        "Optional limit: assert max von Mises stress ≤ this (claim " +
        "physics.static.stress). E.g. yield/safety-factor.",
    },
  },
};

interface PhysicsArgs {
  document_id?: string;
  part?: string;
  domain_box?: { min: [number, number, number]; max: [number, number, number] };
  loads?: StaticAnalysisSpec["loads"];
  supports?: StaticAnalysisSpec["supports"];
  fidelity?: "predict" | "verify";
  resolution?: number;
  youngs_modulus_mpa?: number;
  poisson?: number;
  max_displacement_mm?: number;
  max_von_mises_mpa?: number;
}

const round5 = (v: number) => Number(v.toPrecision(5));

function limitClaim(
  id: string,
  description: string,
  subject: string | undefined,
  limit: number,
  actual: number,
  unit: string,
  basis: "predicted" | "verified",
  converged: boolean,
): ReceiptClaim {
  if (!converged || !Number.isFinite(actual)) {
    return {
      ...unverifiableClaim(id, PHYSICS_DOMAIN, description, ORACLE, "FE solve did not converge"),
      basis,
      subject,
    };
  }
  return {
    id,
    domain: PHYSICS_DOMAIN,
    description,
    subject,
    oracle: ORACLE,
    verdict: actual <= limit ? "pass" : "fail",
    basis,
    predicted: { value: limit, unit },
    measured: { value: round5(actual), unit },
  };
}

export function predictPhysicsTool(
  args: Record<string, unknown>,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = args as PhysicsArgs;

  if (!a.loads?.length) throw new Error("predict_physics: `loads` required");
  if (!a.supports?.length) throw new Error("predict_physics: `supports` required");
  if (!!a.part === !!a.domain_box) {
    throw new Error("predict_physics: pass exactly one of `part` or `domain_box`");
  }
  if (a.part && !a.document_id) {
    throw new Error("predict_physics: `part` requires `document_id`");
  }
  const fidelity = a.fidelity ?? "predict";
  const basis = fidelity === "verify" ? "verified" : "predicted";

  const spec: StaticAnalysisSpec = {
    loads: a.loads,
    supports: a.supports,
    resolution: a.resolution ?? TIER_RESOLUTION[fidelity],
    youngs_modulus_mpa: a.youngs_modulus_mpa,
    poisson: a.poisson,
  };

  let documentId: string | undefined;
  let subject: string | undefined;
  const started = performance.now();
  let result: StaticAnalysisResult;
  if (a.part) {
    documentId = String(a.document_id);
    const doc = getSession(documentId);
    const resolved = resolvePartMesh(doc, engine, a.part);
    subject = `part:${resolved.name ?? a.part}`;
    result = engine.analyzeStaticsMesh(resolved.mesh, spec);
  } else {
    // Box runs are pure computation — no session is touched or minted.
    if (a.document_id) documentId = String(a.document_id);
    const box = a.domain_box!;
    result = engine.analyzeStaticsBox(box.min, box.max, spec);
    subject = "domain_box";
  }
  const solveMs = Math.round(performance.now() - started);

  const claims: ReceiptClaim[] = [];
  if (a.max_displacement_mm !== undefined) {
    claims.push(
      limitClaim(
        "physics.static.displacement",
        `max displacement under load ≤ ${a.max_displacement_mm} mm`,
        subject,
        a.max_displacement_mm,
        result.maxDisplacementMm,
        "mm",
        basis,
        result.converged,
      ),
    );
  }
  if (a.max_von_mises_mpa !== undefined) {
    claims.push(
      limitClaim(
        "physics.static.stress",
        `max von Mises stress ≤ ${a.max_von_mises_mpa} MPa`,
        subject,
        a.max_von_mises_mpa,
        result.maxVonMisesMpa,
        "MPa",
        basis,
        result.converged,
      ),
    );
  }

  const receipt: DesignReceipt | undefined = claims.length
    ? { schema: RECEIPT_SCHEMA, document_id: documentId, claims }
    : undefined;

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            document_id: documentId,
            fidelity,
            basis,
            solve_ms: solveMs,
            analysis: {
              max_displacement_mm: round5(result.maxDisplacementMm),
              max_displacement_at: result.maxDisplacementAt.map(round5),
              max_von_mises_mpa: round5(result.maxVonMisesMpa),
              max_stress_at: result.maxStressAt.map(round5),
              compliance_n_mm: round5(result.compliance),
              grid: result.grid,
              voxel_size_mm: round5(result.voxelSizeMm),
              converged: result.converged,
            },
            ...(receipt
              ? { receipt, summary: summarize(receipt) }
              : {
                  note:
                    "No limits asserted — pass max_displacement_mm and/or " +
                    "max_von_mises_mpa to get receipt claims.",
                }),
            ...(fidelity === "predict"
              ? {
                  next:
                    "Estimates only (basis=predicted → summary.verdict=" +
                    "provisional). Re-run with fidelity=\"verify\" to certify.",
                }
              : {}),
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
    name: "predict_physics",
    pack: null,
    description:
      "Fast static structural analysis (voxel FEA): max displacement, max von Mises stress, and " +
      "compliance for a part or box under world-frame loads (N) and supports, in ~100 ms at " +
      "fidelity=predict. Pass max_displacement_mm / max_von_mises_mpa limits to get receipt " +
      "claims: predict-tier claims carry basis=predicted and roll up as a PROVISIONAL receipt — " +
      "use them to iterate on a design cheaply. When the design settles, re-run with " +
      "fidelity=verify (same solver, fine grid) to upgrade the claims to basis=verified and a " +
      "certifiable pass. Loads/supports are box regions (mm, Z-up); zero-thickness boxes select " +
      "a face. Voxel FEA smears stress concentrations — treat stress as an estimate near fillets.",
    inputSchema: predictPhysicsSchema,
    handler: (args, ctx) => predictPhysicsTool(args, ctx.engine),
    behavior: behavior({}),
  },
];
