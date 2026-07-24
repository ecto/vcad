/**
 * Lattice gauge theory tools — the `vcad-kernel-qcd` crate over MCP.
 *
 * `simulate_lattice_gauge` runs quenched pure-gauge Wilson-action Monte
 * Carlo (SU(2) or SU(3)) and returns plaquette / Wilson-loop /
 * string-tension / Polyakov observables — every number a binned-jackknife
 * mean ± error — plus optional viewport field snapshots (action density,
 * Polyakov field, cooled topological charge) and the static-pair
 * flux-tube profile. Fail-closed: statistics-starved runs refuse to mint
 * `vcad.qcd-claims/1` claims (the fix is more sweeps), and the tool
 * reports why. Claims carry `basis: "predicted"` and cap at Provisional —
 * quenched lattice-units physics, never a statement about physical QCD.
 */

import { behavior, type ToolDef } from "./tool-def.js";

const simSpecSchema = {
  type: "object" as const,
  required: [
    "dims",
    "beta",
    "thermalization_sweeps",
    "measurement_sweeps",
    "overrelax_per_heatbath",
    "bin_size",
    "max_wilson_extent",
    "seed",
    "hot_start",
  ],
  description:
    "Lattice simulation spec. Deterministic per seed. Direction 3 is " +
    "time — shorten it (e.g. [6,6,6,2]) for finite temperature / " +
    "deconfinement studies.",
  properties: {
    gauge: {
      type: "string" as const,
      enum: ["Su2", "Su3"],
      description:
        "Gauge group. Su2 (default): quaternion links, exact " +
        "Kennedy-Pendleton heatbath, fastest. Su3: the real QCD gauge " +
        "group, Cabibbo-Marinari updates, ~6x cost per link.",
    },
    dims: {
      type: "array" as const,
      items: { type: "number" as const },
      minItems: 4,
      maxItems: 4,
      description:
        "Lattice extents [n0,n1,n2,n3], each >= 2. 4^4-8^4 are " +
        "seconds-scale; 12^4+ is where the cost gate starts to bite.",
    },
    beta: {
      type: "number" as const,
      description:
        "Inverse coupling beta = 2N/g^2 (Wilson action). SU(2): strong " +
        "coupling < ~1.5, crossover ~2.2-2.5, weak > 4. SU(3): strong " +
        "< ~4, deconfinement at N_t=2 near 5.1, weak > 7.",
    },
    thermalization_sweeps: {
      type: "number" as const,
      description: "Discarded equilibration sweeps (>= 1). 50 is typical.",
    },
    measurement_sweeps: {
      type: "number" as const,
      description:
        "Measured sweeps. Must give >= 2 full jackknife bins (>= 5 bins " +
        "to mint claims). 100-200 is typical.",
    },
    overrelax_per_heatbath: {
      type: "number" as const,
      description:
        "Overrelaxation sweeps interleaved per heatbath sweep (1-3 " +
        "decorrelates much faster per unit work).",
    },
    bin_size: {
      type: "number" as const,
      description: "Jackknife bin size in measurements (10-20 typical).",
    },
    max_wilson_extent: {
      type: "number" as const,
      description:
        "Largest square Wilson loop W(r,t) to measure; 0 = plaquette " +
        "only. Capped at half the smallest lattice extent.",
    },
    seed: { type: "number" as const },
    hot_start: {
      type: "boolean" as const,
      description: "Start from random links instead of cold (identity).",
    },
    smear: {
      type: "object" as const,
      required: ["alpha", "iterations"],
      description:
        "APE spatial smearing applied to a measurement copy before loop " +
        "observables (lifts the string-tension signal at intermediate " +
        "coupling; plaquette stays unsmeared). alpha 0.5, 2-3 iterations " +
        "is customary.",
      properties: {
        alpha: { type: "number" as const },
        iterations: { type: "number" as const },
      },
    },
    measure_temporal_loops: {
      type: "boolean" as const,
      description:
        "Also measure spatial x temporal Wilson loops and derive the " +
        "static quark potential V(r) (+ Cornell fit when >= 3 points " +
        "resolve).",
    },
    measure_polyakov: {
      type: "boolean" as const,
      description:
        "Measure <|L|>, the Polyakov-loop deconfinement order parameter " +
        "(small = confined, O(1) = deconfined).",
    },
    flux_tube: {
      type: "object" as const,
      required: ["separation"],
      description:
        "Measure the chromoelectric flux-tube profile between a static " +
        "quark-antiquark pair at this separation (lattice units, spatial " +
        "axis 0): the connected (Polyakov-pair x action-density) 3D " +
        "excess field plus the pair correlator whose decay with " +
        "separation IS confinement. Use a short time axis (N_t 2-4).",
      properties: { separation: { type: "number" as const } },
    },
    snapshot_cooling: {
      type: "number" as const,
      description:
        "Export a rendering FieldSnapshot (action density per site + " +
        "complex Polyakov field per spatial site) of the final " +
        "configuration after this many cooling sweeps (0 = raw vacuum " +
        "boil; 20-30 = classical lumps). SU(2) runs with cooling >= 1 " +
        "also report the clover topological charge (near-integer = " +
        "instanton content).",
    },
  },
};

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
    name: "simulate_lattice_gauge",
    pack: null,
    description:
      "Lattice gauge theory Monte Carlo (quenched Wilson action, SU(2) or SU(3)): " +
      "confinement from first principles on a laptop-scale 4D lattice. Returns the " +
      "average plaquette, Wilson loops, Creutz ratios + static potential + Cornell fit " +
      "(the string tension sigma*a^2), the Polyakov deconfinement order parameter, and " +
      "optionally viewport-ready field snapshots (action density, Polyakov field, " +
      "cooled topological charge) and the static-pair flux-tube profile — every " +
      "observable a binned-jackknife mean +- error, bit-deterministic per seed. " +
      "Fail-closed: starved statistics refuse to mint vcad.qcd-claims/1 (raise " +
      "measurement_sweeps); logs require loops resolved >= 3-sigma from zero. Honesty " +
      "bounds ride every claim: quenched (no dynamical fermions), lattice units at " +
      "fixed coupling (no continuum limit) — model physics, not physical-QCD numbers. " +
      "Cost ~ volume x sweeps (x6 for SU(3)); 6^4 with 150 sweeps runs in seconds. " +
      "Recipes: confinement demo = Su2, [6,6,6,4], beta 2.2, flux_tube {separation 2}; " +
      "deconfinement = [6,6,6,2], measure_polyakov, beta scan across ~1.88 (Su2) / " +
      "~5.1 (Su3); instantons = Su2, beta 2.4, snapshot_cooling 30.",
    inputSchema: {
      type: "object" as const,
      required: ["spec"],
      properties: { spec: simSpecSchema },
    },
    handler: (args, ctx) => {
      const a = args as Record<string, unknown>;
      const out = ctx.engine.latticeGaugeSimulate(JSON.stringify(a.spec));
      return textResult(out);
    },
    behavior: behavior({}),
  },
];
