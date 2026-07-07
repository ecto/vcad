/**
 * render_molecule must give the agent eyes like render_view: rasterize the
 * ball-and-stick SVG to a PNG image content block when @resvg/resvg-js is
 * present, and degrade to raw SVG text when it isn't.
 */

import { describe, it, expect } from "vitest";
import type { MoleculeSystem } from "@vcad/ir";
import { renderMolecule } from "../tools/atoms.js";

/** A CO2-ish 3-atom cluster — enough to exercise atoms + bonds. */
function makeMolecule(): MoleculeSystem {
  return {
    species: [
      { element: "C", atomicNumber: 6, mass: 12.011 },
      { element: "O", atomicNumber: 8, mass: 15.999 },
    ],
    positions: [
      [0, 0, 0],
      [1.16, 0, 0],
      [-1.16, 0, 0],
    ],
    speciesIdx: [0, 1, 1],
    bonds: [
      { a: 0, b: 1, order: 2 },
      { a: 0, b: 2, order: 2 },
    ],
  };
}

describe("render_molecule", () => {
  it("returns a PNG image block when resvg is available", async () => {
    const out = await renderMolecule({ molecule: makeMolecule(), width_px: 320 });
    expect(out.isError).toBeFalsy();

    const image = out.content.find((c) => c.type === "image") as
      | { type: "image"; data: string; mimeType: string }
      | undefined;

    if (image) {
      // Rasterizer present — must be a real PNG, not an empty buffer.
      expect(image.mimeType).toBe("image/png");
      const png = Buffer.from(image.data, "base64");
      expect(png.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
      expect(png.length).toBeGreaterThan(500);
      const meta = out.content.find((c) => c.type === "text") as {
        type: "text";
        text: string;
      };
      expect(JSON.parse(meta.text).format).toBe("png");
    } else {
      // Fallback path: raw SVG text with an honest note about why.
      const meta = out.content[0] as { type: "text"; text: string };
      const parsed = JSON.parse(meta.text);
      expect(parsed.format).toBe("svg");
      expect(parsed.svg).toContain("<svg");
      expect(parsed.note).toBeTruthy();
    }
  });

  it("surfaces malformed input as an error result", async () => {
    // A molecule with no positions array trips the handler — must set isError.
    const out = await renderMolecule({ molecule: {} as MoleculeSystem });
    expect(out.isError).toBe(true);
  });
});
