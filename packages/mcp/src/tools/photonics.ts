/**
 * Photonics FDTD tools — the `vcad-kernel-photonics` crate over MCP.
 *
 * `simulate_photonics` runs a forward 2D TM FDTD transmission analysis
 * of a rect-composed device (straight guides, tapers, splitters) with a
 * slab-mode line source and flux monitors, returning the transmission
 * spectrum and the `vcad.photonics-claims/1` set plus unified-receipt
 * claims. Predictions carry `basis: "predicted"` and roll up Provisional
 * — never verified — until the chip is measured.
 *
 * The crate's adjoint inverse-design loop is deliberately NOT exposed
 * here: its device wiring (objectives, arm monitors, beta schedule)
 * lives in example code rather than a library seam. When that seam
 * lands in-crate, an `optimize_photonics` tool follows.
 */

import { behavior, type ToolDef } from "./tool-def.js";

const photonicsSpecSchema = {
  type: "object" as const,
  required: [
    "wavelength_um",
    "n_core",
    "n_clad",
    "size_um",
    "core_rects_um",
    "source",
    "monitor_in_x_um",
    "outputs",
  ],
  description:
    "2D device in the xy plane, 1 unit = 1 um, propagation along +x. " +
    "Cladding fills the domain; `core_rects_um` are painted at n_core^2 " +
    "with sub-pixel averaging. Literal numbers only.",
  properties: {
    wavelength_um: {
      type: "number" as const,
      description: "Vacuum wavelength, um (e.g. 1.55).",
    },
    n_core: { type: "number" as const },
    n_clad: { type: "number" as const },
    size_um: {
      type: "array" as const,
      minItems: 2,
      maxItems: 2,
      items: { type: "number" as const },
      description: "Domain size [lx, ly], um.",
    },
    core_rects_um: {
      type: "array" as const,
      minItems: 1,
      items: {
        type: "array" as const,
        minItems: 4,
        maxItems: 4,
        items: { type: "number" as const },
      },
      description: "Core rectangles [x0, y0, x1, y1], um.",
    },
    source: {
      type: "object" as const,
      required: ["x_um", "half_width_um"],
      description:
        "Slab-mode line source: the even TM mode of a guide with this " +
        "half-width is solved analytically and injected as a line profile.",
      properties: {
        x_um: { type: "number" as const },
        center_y_um: {
          type: "number" as const,
          description: "Mode center. Default domain mid-height.",
        },
        half_width_um: {
          type: "number" as const,
          description: "Guide half-width the source mode is solved for.",
        },
      },
    },
    monitor_in_x_um: {
      type: "number" as const,
      description:
        "Input-power monitor x (between source and device; full-height).",
    },
    outputs: {
      type: "array" as const,
      minItems: 1,
      maxItems: 2,
      items: {
        type: "object" as const,
        required: ["x_um"],
        properties: {
          x_um: { type: "number" as const },
          y_lo_um: {
            type: "number" as const,
            description: "Window low edge. Default full interior height.",
          },
          y_hi_um: {
            type: "number" as const,
            description: "Window high edge. Default full interior height.",
          },
        },
      },
      description:
        "One or two output flux monitors. Two = splitter arms (use y " +
        "windows to separate them); one = simple transmission (arm B " +
        "claims read zero).",
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
    name: "simulate_photonics",
    pack: null,
    description:
      "Forward 2D TM FDTD photonics run (validated Yee grid + CPML, dispersion error " +
      "~5e-8 at 40 cells/lambda): rect-composed device, analytically-solved slab-mode " +
      "line source, input/output flux monitors, transmission spectrum, insertion loss, " +
      "and splitting ratio — as data plus `vcad.photonics-claims/1` and unified-receipt " +
      "claims. Predictions carry basis \"predicted\" and roll up Provisional until the " +
      "chip is measured. 2D TM only: quantitative for the 2D problem, qualitative for a " +
      "3D chip (no out-of-plane loss); lossless dielectrics; rect geometry only in this " +
      "tool. Deterministic. Cost ~grid cells x steps (caps 2M cells / 100k steps); the " +
      "default 20 cells/lambda x 3000 steps on a 10x5 um domain runs in a few seconds. " +
      "Raise `resolution` toward 40 for publication-grade numbers.",
    inputSchema: {
      type: "object" as const,
      required: ["spec"],
      properties: {
        spec: photonicsSpecSchema,
        options: {
          type: "object" as const,
          properties: {
            resolution: {
              type: "number" as const,
              description: "Cells per vacuum wavelength (>= 8). Default 20.",
            },
            steps: {
              type: "number" as const,
              description: "FDTD time steps. Default 3000.",
            },
            cpml_cells: {
              type: "number" as const,
              description: "Absorber thickness per side. Default 12.",
            },
            courant: {
              type: "number" as const,
              description: "Courant factor (0, 1]. Default 0.5.",
            },
            n_freqs: {
              type: "number" as const,
              description:
                "Monitor frequencies (forced odd, center exact). Default 1.",
            },
            band_frac: {
              type: "number" as const,
              description:
                "Fractional bandwidth spanned when n_freqs > 1. Default 0.2.",
            },
          },
        },
      },
    },
    handler: (args, ctx) => {
      const a = args as Json;
      const out = ctx.engine.photonicsSimulate(
        JSON.stringify(a.spec),
        JSON.stringify(a.options ?? {}),
      );
      return textResult(out);
    },
    behavior: behavior({}),
  },
];
