import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { topologyOptimizeTool } from "../tools/topopt.js";
import { documents, getSession, openDocument } from "../tools/session.js";

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

/** Cantilever fixture: anchor the x=0 face, hang a -Z load off the far
 *  lower edge. Small resolution/iterations keep the FE solves test-sized. */
const cantileverArgs = {
  domain_box: { min: [0, 0, 0], max: [24, 6, 12] },
  loads: [
    {
      region: { min: [24, 0, 0], max: [24, 6, 2] },
      force: [0, 0, -100],
    },
  ],
  supports: [{ region: { min: [0, 0, 0], max: [0, 6, 12] } }],
  resolution: 16,
  max_iterations: 8,
  volume_fraction: 0.35,
};

function cubeDocument(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "block",
        op: { type: "Cube", size: { x: 20, y: 8, z: 10 } },
      },
    },
    roots: [{ root: 1, material: "steel" }],
    part_materials: {},
    materials: {},
  } as unknown as Document;
}

describe("topology_optimize", () => {
  it("optimizes a box domain and freezes the result into a new document", () => {
    const result = out(topologyOptimizeTool(cantileverArgs, engine));

    expect(result.document_id).toBeTruthy();
    expect(result.triangles).toBeGreaterThan(100);
    expect(result.iterations).toBeGreaterThanOrEqual(2);
    // The optimizer must have actually stiffened the structure…
    expect(result.compliance.final).toBeLessThan(result.compliance.initial);
    // …while holding the volume budget.
    expect(result.volume_fraction_achieved).toBeGreaterThan(0.3);
    expect(result.volume_fraction_achieved).toBeLessThan(0.4);

    // The optimized part landed in the session document as a frozen mesh.
    const doc = getSession(result.document_id);
    expect(doc.roots).toHaveLength(1);
    const node = doc.nodes[result.part_id];
    expect(node.op.type).toBe("ImportedMesh");
  });

  it("lightweights an existing part and hides the source", () => {
    const opened = out(openDocument({ initial: cubeDocument() }));
    const result = out(
      topologyOptimizeTool(
        {
          document_id: opened.document_id,
          part: "block",
          loads: [
            {
              region: { min: [20, 0, 0], max: [20, 8, 2] },
              force: [0, 0, -50],
            },
          ],
          supports: [{ region: { min: [0, 0, 0], max: [0, 8, 10] } }],
          resolution: 14,
          max_iterations: 6,
          volume_fraction: 0.4,
        },
        engine,
      ),
    );

    expect(result.document_id).toBe(opened.document_id);
    expect(result.name).toBe("block (optimized)");
    expect(result.source_part_hidden).toBe("block");

    const doc = getSession(opened.document_id);
    expect(doc.roots).toHaveLength(2);
    expect(doc.roots[0].visible).toBe(false); // source hidden
    expect(doc.roots[1].material).toBe("steel"); // material carried over

    // Optimized geometry stays inside the source part's bounding box.
    const node = doc.nodes[result.part_id];
    expect(node.op.type).toBe("ImportedMesh");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const positions = (node.op as any).positions as number[];
    for (let i = 0; i < positions.length; i += 3) {
      expect(positions[i]).toBeGreaterThanOrEqual(-0.5);
      expect(positions[i]).toBeLessThanOrEqual(20.5);
      expect(positions[i + 2]).toBeGreaterThanOrEqual(-0.5);
      expect(positions[i + 2]).toBeLessThanOrEqual(10.5);
    }
  });

  it("rejects ambiguous or incomplete specs", () => {
    expect(() =>
      topologyOptimizeTool({ ...cantileverArgs, loads: [] }, engine),
    ).toThrow(/loads/);
    expect(() =>
      topologyOptimizeTool({ ...cantileverArgs, part: "x" }, engine),
    ).toThrow(/exactly one/);
    expect(() =>
      topologyOptimizeTool(
        { ...cantileverArgs, domain_box: undefined, part: "x" },
        engine,
      ),
    ).toThrow(/document_id/);
  });

  it("errors when boundary conditions miss the domain", () => {
    expect(() =>
      topologyOptimizeTool(
        {
          ...cantileverArgs,
          loads: [
            {
              region: { min: [500, 500, 500], max: [501, 501, 501] },
              force: [0, 0, -1],
            },
          ],
        },
        engine,
      ),
    ).toThrow(/load/i);
  });
});
