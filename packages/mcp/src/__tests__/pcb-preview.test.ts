import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { createSchematic, placeComponents, routeNets } from "../tools/ecad.js";
import { documents, getSession } from "../tools/session.js";
import { generateGlbPreview } from "../tools/preview.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

const resistor = (ref: string, x: number) => ({
  ref,
  value: "1k",
  footprint: "Resistor_SMD:R_0805",
  x,
  y: 0,
  pins: [
    { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
    { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
  ],
});

/** Parse the JSON chunk out of a binary GLB. */
function parseGlbJson(b64: string): {
  materials: Array<{
    pbrMetallicRoughness?: { baseColorFactor?: number[] };
    extensions?: Record<string, unknown>;
  }>;
  meshes: Array<{ primitives: Array<{ attributes: Record<string, number> }> }>;
  nodes: Array<{ name?: string }>;
  extensionsUsed?: string[];
} {
  const bytes = Buffer.from(b64, "base64");
  // 12-byte header, then chunk: [u32 len][u32 type][data].
  const jsonLen = bytes.readUInt32LE(12);
  const jsonType = bytes.readUInt32LE(16);
  expect(jsonType).toBe(0x4e4f534a); // "JSON"
  const json = bytes.subarray(20, 20 + jsonLen).toString("utf8");
  return JSON.parse(json);
}

const near = (a: number[] | undefined, b: number[], eps = 0.02): boolean =>
  !!a && b.every((v, i) => Math.abs((a[i] ?? -99) - v) < eps);

describe("PCB GLB preview", () => {
  it("renders a populated board as layered, colored meshes (not one gray slab)", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0), resistor("R2", 20)],
        nets: { MID: ["R1.2", "R2.1"] },
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 50, board_height: 50 }));
    out(await routeNets({ document_id: id }));

    const b64 = await generateGlbPreview(getSession(id), engine);
    expect(b64).toBeTruthy();
    const gltf = parseGlbJson(b64!);

    // More than one mesh — the board is no longer a single merged body.
    expect(gltf.meshes.length).toBeGreaterThan(1);

    // Every primitive carries normals for proper shading.
    for (const m of gltf.meshes) {
      expect(m.primitives[0].attributes.NORMAL).toBeDefined();
      expect(m.primitives[0].attributes.POSITION).toBeDefined();
    }

    // Distinct PBR colors: glossy green soldermask, matte-tan FR4 edge, ENIG copper.
    const colors = gltf.materials.map((m) => m.pbrMetallicRoughness?.baseColorFactor);
    const hasMask = colors.some((c) => near(c, [0.045, 0.21, 0.1]));
    const hasSubstrate = colors.some((c) => near(c, [0.46, 0.38, 0.22]));
    const hasCopper = colors.some((c) => near(c, [0.85, 0.66, 0.3]));
    expect(hasMask, `colors: ${JSON.stringify(colors)}`).toBe(true);
    expect(hasSubstrate, `colors: ${JSON.stringify(colors)}`).toBe(true);
    expect(hasCopper, `colors: ${JSON.stringify(colors)}`).toBe(true);

    // No material is the old neutral gray default for the board.
    const allGray = colors.every((c) => near(c, [0.8, 0.8, 0.8]));
    expect(allGray).toBe(false);

    // The glossy soldermask carries a KHR_materials_clearcoat extension.
    expect(gltf.extensionsUsed ?? []).toContain("KHR_materials_clearcoat");
    expect(
      gltf.materials.some((m) => m.extensions?.KHR_materials_clearcoat),
    ).toBe(true);

    // Board sub-meshes keep the PcbBoard part identity for click-to-select.
    expect(gltf.nodes.some((n) => (n.name ?? "").includes("PCB Board"))).toBe(true);
  });

  it("a bare board still previews (substrate only)", async () => {
    const created = out(
      await createSchematic({
        components: [resistor("R1", 0)],
        nets: {},
      }),
    );
    const id = created.document_id;
    out(await placeComponents({ document_id: id, board_width: 30, board_height: 30 }));

    const b64 = await generateGlbPreview(getSession(id), engine);
    expect(b64).toBeTruthy();
    const gltf = parseGlbJson(b64!);
    const colors = gltf.materials.map((m) => m.pbrMetallicRoughness?.baseColorFactor);
    expect(colors.some((c) => near(c, [0.045, 0.21, 0.1]))).toBe(true);
  });
});
