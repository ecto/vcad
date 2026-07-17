/**
 * Neutronics tools — the `vcad-kernel-neutronics` crate over MCP.
 *
 * `simulate_neutron_shield` runs analog Monte Carlo neutron transport
 * through a spherical shield stack and returns dose rates at detector
 * shells WITH batch-statistics error bars, plus the
 * `vcad.neutronics-claims/1` set and unified-receipt claims. Fail-closed
 * throughout: truncated histories or unscored tallies refuse to price
 * claims (the fix is more histories). Predictions carry
 * `basis: "predicted"` and roll up Provisional — never verified — until
 * a survey meter measurement is bound.
 */

import { behavior, type ToolDef } from "./tool-def.js";

const paramValue = {
  description:
    "A number, or the name of a named parameter bound via `parameters`.",
  anyOf: [{ type: "number" as const }, { type: "string" as const }],
};

const shieldSpecSchema = {
  type: "object" as const,
  required: ["layers", "source", "detectors"],
  description:
    "Spherical shield: concentric layers from r=0 outward around a point " +
    "source, with detector shells that must each sit strictly inside one " +
    "layer (put them in air gaps). Units mm. Any numeric field may " +
    "instead be a string naming a parameter supplied in `parameters` " +
    "(fail-closed: unbound names error).",
  properties: {
    layers: {
      type: "array" as const,
      minItems: 1,
      items: {
        type: "object" as const,
        required: ["material", "thickness_mm"],
        properties: {
          material: {
            type: "string" as const,
            description:
              "Built-in library material: hdpe, paraffin, borated-hdpe-5, " +
              "water, lead, concrete, air, void.",
          },
          thickness_mm: paramValue,
        },
      },
    },
    source: {
      type: "object" as const,
      required: ["rate_n_per_s", "energy_ev"],
      description:
        "Isotropic point source at r=0. Energy must lie in the 5-group " +
        "structure [1e-4, 3e6] eV — D-D 2.45e6 eV is in range; D-T 14.1 " +
        "MeV is rejected.",
      properties: {
        rate_n_per_s: paramValue,
        energy_ev: paramValue,
      },
    },
    detectors: {
      type: "array" as const,
      minItems: 1,
      items: {
        type: "object" as const,
        required: ["label", "radius_mm"],
        properties: {
          label: { type: "string" as const },
          radius_mm: paramValue,
          half_width_mm: {
            ...paramValue,
            description: "Shell half-thickness. Default 20 mm.",
          },
        },
      },
    },
    run: {
      type: "object" as const,
      description: "Monte Carlo controls (deterministic for a given seed).",
      properties: {
        histories_per_batch: {
          type: "number" as const,
          description: "Default 20000.",
        },
        batches: {
          type: "number" as const,
          description: "Error-bar batches (>= 2). Default 20.",
        },
        seed: { type: "number" as const, description: "Default 20260717." },
      },
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
    name: "simulate_neutron_shield",
    pack: null,
    description:
      "Monte Carlo neutron shielding run (analog transport, 5-group multigroup library " +
      "with exact elastic kinematics): spherical layer stack around a point source (D-D " +
      "band), H*(10) dose rate at each detector shell WITH 1-sigma batch error bars " +
      "(rse), attenuation factor vs bare source, thermal flux, and absorbed/leakage " +
      "balance — as data plus `vcad.neutronics-claims/1` and unified-receipt claims. " +
      "Fail-closed: truncated histories or a tally that scored nothing refuses claims " +
      "(raise histories). Predictions carry basis \"predicted\" and roll up Provisional " +
      "until surveyed. Design-estimate physics: 5-group library (not ENDF continuous-" +
      "energy), spherical geometry only, no gamma dose, no activation, source energy " +
      "capped at 3 MeV (no D-T). Deterministic per seed. Cost ~histories (cap 5M); " +
      "error bars shrink as 1/sqrt(N). The 400k default runs in seconds.",
    inputSchema: {
      type: "object" as const,
      required: ["spec"],
      properties: {
        spec: shieldSpecSchema,
        parameters: {
          type: "object" as const,
          additionalProperties: { type: "number" as const },
          description: "Bindings for named spec parameters, `{name: value}`.",
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as Json;
      const out = ctx.engine.neutronicsSimulate(
        JSON.stringify(a.spec),
        JSON.stringify(a.parameters ?? {}),
      );
      return textResult(out);
    },
    behavior: behavior({}),
  },
];
