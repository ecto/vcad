/**
 * Fluid-flow tools — the `vcad-kernel-flow` crate over MCP.
 *
 * `simulate_flow` runs a steady laminar D3Q19 BGK lattice-Boltzmann
 * solve on a voxel grid and returns pressure drop, flow rates, the mass
 * audit, optional thermal pickup, and the `vcad.flow-claims/1` set plus
 * unified-receipt claims. Predictions carry `basis: "predicted"` and
 * roll up Provisional — never verified — until hardware is measured.
 */

import { behavior, type ToolDef } from "./tool-def.js";

const vec3 = {
  type: "array" as const,
  minItems: 3,
  maxItems: 3,
  items: { type: "number" as const },
};

const shapeSchema = {
  type: "object" as const,
  required: ["type"],
  description:
    'Region shape: `{"type":"Box","min_mm":[..],"size_mm":[..]}` or ' +
    '`{"type":"Tube","axis":"Z","center_mm":[..2],"span_mm":[..2],' +
    '"outer_radius_mm":..,"inner_radius_mm":..}`.',
  properties: {
    type: { type: "string" as const, enum: ["Box", "Tube"] },
    min_mm: vec3,
    size_mm: vec3,
    axis: { type: "string" as const, enum: ["X", "Y", "Z"] },
    center_mm: {
      type: "array" as const,
      minItems: 2,
      maxItems: 2,
      items: { type: "number" as const },
    },
    span_mm: {
      type: "array" as const,
      minItems: 2,
      maxItems: 2,
      items: { type: "number" as const },
    },
    outer_radius_mm: { type: "number" as const },
    inner_radius_mm: { type: "number" as const },
  },
};

const flowSpecSchema = {
  type: "object" as const,
  required: ["origin_mm", "size_mm", "divisions"],
  description:
    "Voxelized flow domain: an axis-aligned box discretized into cubic " +
    "voxels (`size_mm[a]/divisions[a]` must agree across axes), painted " +
    "with regions in order (`background` first, then `regions`). Units " +
    "mm / kg / m / s / Pa / degC.",
  properties: {
    origin_mm: vec3,
    size_mm: vec3,
    divisions: {
      type: "array" as const,
      minItems: 3,
      maxItems: 3,
      items: { type: "number" as const },
      description:
        "Voxel counts per axis (voxels must come out cubic; the cost knob).",
    },
    background: {
      type: "string" as const,
      enum: ["solid", "fluid"],
      description: "What unpainted voxels are. Default solid.",
    },
    regions: {
      type: "array" as const,
      items: {
        type: "object" as const,
        required: ["shape", "role"],
        properties: {
          shape: shapeSchema,
          role: {
            type: "string" as const,
            enum: ["solid", "fluid", "inlet", "outlet"],
          },
        },
      },
      description:
        "Painted in order; later regions override earlier ones. Inlet " +
        "and outlet patches must sit on the domain boundary.",
    },
    fluid: {
      type: "object" as const,
      properties: {
        density_kg_m3: { type: "number" as const },
        viscosity_pa_s: { type: "number" as const },
      },
      description: "Fluid properties. Default water (998, 1.002e-3).",
    },
    inlet_velocity_m_s: {
      ...vec3,
      description: "Plug-flow inlet velocity vector, m/s.",
    },
    outlet_gauge_pa: {
      type: "number" as const,
      description: "Outlet gauge pressure, Pa. Default 0.",
    },
    body_force_n_m3: {
      ...vec3,
      description:
        "Volumetric body force, N/m^3 (periodic channel drive; needs " +
        "`options.u_ref_m_s`).",
    },
    periodic: {
      type: "array" as const,
      minItems: 3,
      maxItems: 3,
      items: { type: "boolean" as const },
      description: "Per-axis periodic boundaries.",
    },
    re_envelope: {
      type: "number" as const,
      description:
        "Reynolds gate: the solve refuses Re above this (laminar only). " +
        "Default 2300.",
    },
    thermal: {
      type: "object" as const,
      properties: {
        inlet_temp_c: { type: "number" as const },
        initial_temp_c: { type: "number" as const },
        wall_temp_c: { type: "number" as const },
        diffusivity_m2_s: { type: "number" as const },
        heat_capacity_j_kg_k: { type: "number" as const },
        buoyancy: {
          type: "object" as const,
          properties: {
            beta_per_k: { type: "number" as const },
            t_ref_c: { type: "number" as const },
            gravity_m_s2: { type: "number" as const },
          },
        },
      },
      description:
        "Enable the advected temperature field (film-averaged conjugate " +
        "seam; optional Boussinesq buoyancy, gated at Ra <= 1e8).",
    },
    hot_walls: {
      type: "array" as const,
      items: {
        type: "object" as const,
        required: ["shape", "temp_c"],
        properties: {
          shape: shapeSchema,
          temp_c: { type: "number" as const },
        },
      },
      description:
        "Isothermal wall patches (Dirichlet) for thermal runs, painted " +
        "over solid voxels adjacent to fluid.",
    },
  },
};

type Json = Record<string, unknown>;

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
    name: "simulate_flow",
    pack: null,
    description:
      "Steady laminar CFD on a voxel grid: D3Q19 BGK lattice Boltzmann with " +
      "half-way bounce-back walls, velocity inlets, pressure outlets, and an " +
      "optional advected temperature field (isothermal walls, Boussinesq " +
      "buoyancy). Returns pressure drop, inlet/outlet flow rates, the mass-" +
      "balance audit, max speed, and thermal pickup — as data plus " +
      "`vcad.flow-claims/1` and unified-receipt claims. Predictions carry basis " +
      '"predicted" and roll up Provisional until hardware is measured. Laminar ' +
      "only: fail-closed gates at Re <= 2300, the stable tau window, and " +
      "Ra <= 1e8; weakly compressible (pressure noise O(Ma^2)); walls are voxel " +
      "staircases. Per-voxel velocity/pressure/temperature fields are only " +
      "returned with `include_fields: true` — they are grid-sized (voxel cap 2M). " +
      "Steadiness is detected, not assumed: a run that never goes steady is an " +
      "error, not a result.",
    inputSchema: {
      type: "object" as const,
      required: ["spec"],
      properties: {
        spec: flowSpecSchema,
        options: {
          type: "object" as const,
          properties: {
            u_ref_m_s: {
              type: "number" as const,
              description:
                "Reference speed for unit scaling, m/s. Defaults to the " +
                "inlet speed; body-force-driven cases must supply it.",
            },
            max_steps: {
              type: "number" as const,
              description: "Step budget before failing. Default 400000.",
            },
            check_every: {
              type: "number" as const,
              description: "Steadiness check interval, steps. Default 200.",
            },
            steady_tol: {
              type: "number" as const,
              description:
                "Relative L-inf velocity change per check. Default 1e-6.",
            },
            ramp_steps: {
              type: "number" as const,
              description: "Inlet velocity ramp length, steps. Default 1000.",
            },
          },
        },
        include_fields: {
          type: "boolean" as const,
          description:
            "Return the per-voxel velocity/pressure/temperature fields " +
            "(grid-sized). Default false.",
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as Json;
      // The kernel loop is chunked (a budget of LBM timesteps per call), so
      // progress notifications flush between chunks instead of bursting at
      // the end. Bit-identical to the one-shot path.
      const out = ctx.engine.simulateFlowChunked(
        JSON.stringify(a.spec),
        JSON.stringify(a.options ?? {}),
        !!a.include_fields,
        ctx.progress
          ? (s) =>
              ctx.progress?.(
                Math.min(s.steps, s.max_steps),
                s.max_steps,
                s.converged
                  ? `LBM steady after ${s.steps} steps (residual ${s.residual.toExponential(2)})`
                  : `LBM step ${s.steps}/${s.max_steps}, residual ${
                      Number.isFinite(s.residual) ? s.residual.toExponential(2) : "—"
                    }`,
              )
          : undefined,
      );
      return textResult(out);
    },
    behavior: behavior({}),
  },
];
