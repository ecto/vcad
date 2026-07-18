/**
 * Circuit simulation tools — `vcad-ecad-sim::circuit` over MCP.
 *
 * `simulate_circuit` runs DC operating point / small-signal AC / transient
 * analyses of a lumped-element netlist (SPICE-style MNA), returning node
 * voltages, currents, Bode points, and `vcad.spice-claims/1` claims plus
 * unified-receipt claims. The Tellegen power-balance residual rides along as
 * the honesty signal. `tune_circuit` drives free component values toward a
 * filter (cutoff/Q) or DC-voltage target with adjoint gradients (one
 * transposed solve per probe — the whole gradient regardless of component
 * count). Predictions carry `basis: "predicted"` and roll up Provisional —
 * never verified — until the hardware is measured.
 */

import { behavior, type ToolDef } from "./tool-def.js";

const deviceSchema = {
  type: "object" as const,
  required: ["kind", "p", "n"],
  properties: {
    kind: {
      type: "string" as const,
      enum: [
        "resistor",
        "capacitor",
        "inductor",
        "vsource",
        "isource",
        "diode",
        "led",
        "motor",
      ],
      description:
        "Device kind. `diode`/`led` use built-in Shockley models; `motor` a " +
        "small-DC model (transient only).",
    },
    p: { type: "number" as const, description: "Positive node id (0 = ground)." },
    n: { type: "number" as const, description: "Negative node id (0 = ground)." },
    value: {
      type: "number" as const,
      description:
        "Primary value: R in ohms, C in farads, L in henries, V in volts, " +
        "I in amps. Ignored for diode/led/motor.",
    },
  },
};

const devicesSchema = {
  type: "array" as const,
  minItems: 1,
  items: deviceSchema,
  description:
    "Netlist as data. Node ids are dense integers with 0 = ground; the " +
    "device's index in this array is its device id (used by sourceId / " +
    "freeDevices / outNode references).",
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

/** Frequency grid: explicit list, or a log-spaced start/stop/points sweep. */
function frequencyGrid(ac: Json): number[] {
  const list = ac.frequenciesHz as number[] | undefined;
  if (Array.isArray(list) && list.length > 0) return list;
  const start = (ac.startHz as number) ?? 10;
  const stop = (ac.stopHz as number) ?? 1e6;
  const points = Math.min(Math.max((ac.points as number) ?? 40, 2), 500);
  if (!(start > 0) || !(stop > start)) {
    throw new Error("ac sweep needs 0 < startHz < stopHz");
  }
  const out: number[] = [];
  for (let i = 0; i < points; i++) {
    out.push(start * Math.pow(stop / start, i / (points - 1)));
  }
  return out;
}

export const toolDefs: ToolDef[] = [
  {
    name: "simulate_circuit",
    pack: null,
    description:
      "SPICE-style lumped-element circuit simulation (MNA): DC operating point " +
      "(gmin-stepped Newton, exact at gmin=0), small-signal AC sweep (complex MNA, " +
      "per-frequency complex node voltages + Bode magnitude/phase for outNode), and " +
      "transient (trapezoidal, SPICE2's integrator). Netlist is data: " +
      "{devices:[{kind,p,n,value}]}, node 0 = ground, device id = array index. " +
      "Returns node voltages/currents and `vcad.spice-claims/1` + unified-receipt " +
      "claims (basis \"predicted\", rolls up Provisional until measured — a $30 USB " +
      "scope + signal generator close them). The Tellegen power-balance residual is " +
      "reported as the honesty signal: it is solver error and nothing else. Limits: " +
      "diode AC uses the DC-linearized small-signal model; motors are transient-only.",
    inputSchema: {
      type: "object" as const,
      required: ["devices", "analyses"],
      properties: {
        devices: devicesSchema,
        analyses: {
          type: "array" as const,
          minItems: 1,
          items: { type: "string" as const, enum: ["dc", "ac", "transient"] },
          description: "Which analyses to run.",
        },
        ac: {
          type: "object" as const,
          properties: {
            sourceId: {
              type: "number" as const,
              description: "Device id of the driving V/I source (unit amplitude).",
            },
            outNode: {
              type: "number" as const,
              description:
                "Node whose transfer magnitude/phase to tabulate (Bode points).",
            },
            frequenciesHz: {
              type: "array" as const,
              items: { type: "number" as const },
              description: "Explicit frequency list (Hz). Overrides the sweep.",
            },
            startHz: { type: "number" as const, description: "Sweep start (default 10)." },
            stopHz: { type: "number" as const, description: "Sweep stop (default 1e6)." },
            points: {
              type: "number" as const,
              description: "Log-spaced sweep points (default 40, max 500).",
            },
          },
          description: "Required when analyses includes \"ac\".",
        },
        transient: {
          type: "object" as const,
          properties: {
            dt: { type: "number" as const, description: "Timestep (s)." },
            steps: { type: "number" as const, description: "Step count (max 2e6)." },
            sampleEvery: {
              type: "number" as const,
              description: "Record every Nth step (sample cap 5000). Default 1.",
            },
          },
          description: "Required when analyses includes \"transient\".",
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as Json;
      const analyses = a.analyses as string[];
      const specJson = JSON.stringify({
        dt: (a.transient as Json | undefined)?.dt ?? 0,
        devices: a.devices,
      });
      const out: Json = {};
      if (analyses.includes("dc")) {
        out.dc = ctx.engine.circuitDcOperatingPoint(specJson);
      }
      if (analyses.includes("ac")) {
        const ac = (a.ac ?? {}) as Json;
        if (typeof ac.sourceId !== "number") {
          throw new Error('analyses includes "ac" but ac.sourceId is missing');
        }
        const freqs = frequencyGrid(ac);
        const omegas = freqs.map((f) => 2 * Math.PI * f);
        const res = ctx.engine.circuitAcResponse(
          specJson,
          ac.sourceId as number,
          omegas,
        ) as {
          points: Array<{
            omega: number;
            nodeVoltagesRe: number[];
            nodeVoltagesIm: number[];
          }>;
        };
        const outNode = ac.outNode as number | undefined;
        out.ac = {
          sourceId: ac.sourceId,
          frequenciesHz: freqs,
          points: res.points,
          ...(typeof outNode === "number"
            ? {
                bode: res.points.map((p, i) => {
                  const re = p.nodeVoltagesRe[outNode];
                  const im = p.nodeVoltagesIm[outNode];
                  return {
                    frequencyHz: freqs[i],
                    magnitude: Math.hypot(re, im),
                    phaseDeg: (Math.atan2(im, re) * 180) / Math.PI,
                  };
                }),
              }
            : {}),
        };
      }
      if (analyses.includes("transient")) {
        const tr = (a.transient ?? {}) as Json;
        if (typeof tr.dt !== "number" || typeof tr.steps !== "number") {
          throw new Error(
            'analyses includes "transient" but transient.dt/steps are missing',
          );
        }
        out.transient = ctx.engine.circuitTransient(
          specJson,
          tr.steps as number,
          (tr.sampleEvery as number) ?? 1,
        );
      }
      return textResult(out);
    },
    behavior: behavior({}),
  },
  {
    name: "tune_circuit",
    pack: null,
    description:
      "Adjoint-driven circuit tuning: gradient descent in log-parameter space " +
      "(backtracking line search, scale-invariant stop) moves the freeDevices' " +
      "values toward a target — either a 2nd-order filter target {cutoffHz, qFactor, " +
      "sourceId, outNode} or a DC target {node, dcVoltage}. Each gradient costs one " +
      "transposed MNA solve per probe regardless of component count. Returns tuned " +
      "values, before/after response points, iteration count, the achieved target " +
      "(cutoff measured by −3 dB bisection, Q from the −90° phase crossing, or the " +
      "achieved DC voltage), and `vcad.spice-claims/1` + unified-receipt claims " +
      "(predicted basis, Provisional rollup). Fails closed if a free device's " +
      "sensitivity is a deferred placeholder (diodes at AC) or non-positive " +
      "(log-space needs positive values).",
    inputSchema: {
      type: "object" as const,
      required: ["devices", "freeDevices", "target"],
      properties: {
        devices: devicesSchema,
        target: {
          type: "object" as const,
          description:
            "Either a filter target {cutoffHz, qFactor, sourceId, outNode} " +
            "(sourceId = driving source device id, outNode = response node) or a " +
            "DC target {node, dcVoltage}.",
          properties: {
            cutoffHz: { type: "number" as const },
            qFactor: { type: "number" as const },
            sourceId: { type: "number" as const },
            outNode: { type: "number" as const },
            node: { type: "number" as const },
            dcVoltage: { type: "number" as const },
          },
        },
        freeDevices: {
          type: "array" as const,
          minItems: 1,
          items: {
            type: "object" as const,
            required: ["device"],
            properties: {
              device: {
                type: "number" as const,
                description: "Device id (index into devices) allowed to move.",
              },
              min: { type: "number" as const, description: "Lower bound (> 0)." },
              max: { type: "number" as const, description: "Upper bound." },
            },
          },
        },
        maxIters: {
          type: "number" as const,
          description: "Gradient iteration cap (default 500, max 5000).",
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as Json;
      const target = a.target as Json;
      const isFilter =
        typeof target.cutoffHz === "number" || typeof target.qFactor === "number";
      const isDc =
        typeof target.node === "number" || typeof target.dcVoltage === "number";
      if (isFilter === isDc) {
        throw new Error(
          "target must be exactly one of {cutoffHz, qFactor, sourceId, outNode} or {node, dcVoltage}",
        );
      }
      const tune = {
        ...(isFilter ? { filter: target } : { dc: target }),
        freeDevices: a.freeDevices,
        ...(typeof a.maxIters === "number" ? { maxIters: a.maxIters } : {}),
      };
      const specJson = JSON.stringify({ devices: a.devices });
      return textResult(ctx.engine.circuitTune(specJson, JSON.stringify(tune)));
    },
    behavior: behavior({}),
  },
];
