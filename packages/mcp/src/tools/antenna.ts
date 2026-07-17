/**
 * Antenna solver tools — the `vcad-kernel-antenna` crate over MCP.
 *
 * `analyze_antenna` runs the thin-wire method-of-moments solver over a
 * frequency band: Z_in/S11 sweep, in-band resonance, far-field peak
 * gain, radiation-efficiency cross-check, and the
 * `vcad.antenna-claims/1` set plus unified-receipt claims. Predictions
 * carry `basis: "predicted"` and roll up Provisional — never verified —
 * until a VNA measurement is bound.
 */

import { behavior, type ToolDef } from "./tool-def.js";

const paramValue = {
  description:
    "A number, or the name of a named parameter bound via `parameters`.",
  anyOf: [{ type: "number" as const }, { type: "string" as const }],
};

const pointSpec = {
  type: "array" as const,
  minItems: 3,
  maxItems: 3,
  items: paramValue,
  description: "[x, y, z] in mm (Z-up; ground plane, when on, is z=0).",
};

const antennaSpecSchema = {
  type: "object" as const,
  required: ["elements", "feed_mm"],
  description:
    "Thin-wire antenna: straight wires, polyline paths, and closed loops, " +
    "with a delta-gap feed at the basis nearest `feed_mm`. Units mm. Any " +
    "numeric field may instead be a string naming a parameter supplied in " +
    "`parameters` (fail-closed: unbound names error).",
  properties: {
    ground_plane: {
      type: "boolean" as const,
      description:
        "Infinite PEC ground at z=0 (monopoles). Default false (free space).",
    },
    elements: {
      type: "array" as const,
      minItems: 1,
      items: {
        type: "object" as const,
        required: ["type"],
        description:
          'Element: `{"type":"wire","start_mm","end_mm","radius_mm",' +
          '"segments"}`, `{"type":"path","points_mm","radius_mm",' +
          '"segments_per_leg"}` (len = points-1), or `{"type":"loop",...}` ' +
          "(segments_per_leg len = points, closing leg included).",
        properties: {
          type: { type: "string" as const, enum: ["wire", "path", "loop"] },
          start_mm: pointSpec,
          end_mm: pointSpec,
          points_mm: { type: "array" as const, items: pointSpec },
          radius_mm: paramValue,
          segments: { type: "number" as const },
          segments_per_leg: {
            type: "array" as const,
            items: { type: "number" as const },
          },
        },
      },
    },
    feed_mm: pointSpec,
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
    name: "analyze_antenna",
    pack: null,
    description:
      "Thin-wire method-of-moments antenna analysis over a frequency band: per-frequency " +
      "Z_in and S11 sweep rows, minimum-S11 point, in-band resonance (Im Z = 0), -10 dB " +
      "bandwidth, far-field peak gain (37x72 pattern scan), and a radiation-efficiency " +
      "energy cross-check — as data plus `vcad.antenna-claims/1` and unified-receipt " +
      "claims. Predictions carry basis \"predicted\" and roll up Provisional until a VNA " +
      "sweep is bound. Free-space PEC wires (optional infinite ground plane): no " +
      "dielectric substrate (PCB antennas read a few percent high in frequency), no " +
      "conductor loss (gain = directivity), thin-wire validity gates enforced " +
      "fail-closed (segment >= 4a, segment <= lambda/8, ka <= 0.1). Deterministic. Cost " +
      "~O(segments^3) per frequency x band points (segment cap 600); a 30-segment " +
      "dipole over 46 points runs in seconds.",
    inputSchema: {
      type: "object" as const,
      required: ["spec", "band"],
      properties: {
        spec: antennaSpecSchema,
        parameters: {
          type: "object" as const,
          additionalProperties: { type: "number" as const },
          description: "Bindings for named spec parameters, `{name: value}`.",
        },
        band: {
          type: "object" as const,
          required: ["f_lo_hz", "f_hi_hz", "points"],
          description: "Frequency band to sweep and make claims over.",
          properties: {
            f_lo_hz: { type: "number" as const },
            f_hi_hz: { type: "number" as const },
            points: {
              type: "number" as const,
              description: "Sweep points across the band (2..2000).",
            },
          },
        },
        options: {
          type: "object" as const,
          properties: {
            z0: {
              type: "number" as const,
              description: "S11 reference impedance, ohm. Default 50.",
            },
            quad_outer: {
              type: "number" as const,
              description: "Outer Gauss-Legendre order. Default 6.",
            },
            quad_inner: {
              type: "number" as const,
              description: "Inner Gauss-Legendre order. Default 6.",
            },
            sweep: {
              type: "boolean" as const,
              description:
                "Return per-frequency sweep rows (Z_in, S11). Default true.",
            },
          },
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as Json;
      const out = ctx.engine.antennaAnalyze(
        JSON.stringify(a.spec),
        JSON.stringify(a.parameters ?? {}),
        JSON.stringify({ band: a.band, ...(a.options as Json | undefined) }),
      );
      return textResult(out);
    },
    behavior: behavior({}),
  },
];
