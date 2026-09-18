import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from "vitest";
import type { Document, Node } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { getKernelWasm, resetKernelWasm } from "../wasm-singleton.js";
import { evaluateDocument } from "../evaluate.js";

/**
 * A chain of `Difference` nodes must evaluate IN THE KERNEL, in the browser
 * and in node.
 *
 * The evaluator batches such a chain — one difference against the union of
 * the tools — under a budget it used to read from `std::time::Instant::now()`.
 * That call is not implemented on `wasm32-unknown-unknown`: it panics, which
 * traps the module. `evaluateDocument` below catches the trap and re-evaluates
 * in TypeScript, so the only symptom was one console line, a lost B-rep, and
 * the kernel's speed gone. Anything with no fallback — `camOutlineFromDocument`
 * — simply failed.
 *
 * So the assertion is not "a mesh came back" (the TS fallback produces one
 * too): it is that the kernel binding did not trap, that the engine never
 * announced a fallback, and that the part it returned is the right solid.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */
let wasm: any;

beforeAll(async () => {
  wasm = await getKernelWasm();
});

let warn: ReturnType<typeof vi.spyOn>;

beforeEach(async () => {
  // A trap poisons the instance; start each test on a healthy one so a
  // failure here is this test's own and not the previous one's debris.
  resetKernelWasm("chained-difference test isolation");
  wasm = await getKernelWasm();
  warn = vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  warn.mockRestore();
});

const node = (id: number, op: Node["op"]): Node => ({ id, name: `n${id}`, op });

/**
 * `plate - bore - hole_a - hole_b`, authored as a chain: a 80x50x6 plate, a
 * Ø16 bore through the middle, two Ø5 holes. One difference against a union
 * of the three tools would not reach the batching path at all.
 */
function chainedDifferenceDoc(): Document {
  const doc = createDocument();
  const nodes: Node[] = [
    node(1, { type: "Cube", size: { x: 80, y: 50, z: 6 } }),
    node(2, { type: "Cylinder", radius: 8, height: 8, segments: 96 }),
    node(3, { type: "Translate", child: 2, offset: { x: 40, y: 25, z: -1 } }),
    node(4, { type: "Cylinder", radius: 2.5, height: 8, segments: 64 }),
    node(5, { type: "Translate", child: 4, offset: { x: 12, y: 12, z: -1 } }),
    node(6, { type: "Cylinder", radius: 2.5, height: 8, segments: 64 }),
    node(7, { type: "Translate", child: 6, offset: { x: 68, y: 38, z: -1 } }),
    node(8, { type: "Difference", left: 1, right: 3 }),
    node(9, { type: "Difference", left: 8, right: 5 }),
    node(10, { type: "Difference", left: 9, right: 7 }),
  ] as Node[];
  for (const n of nodes) doc.nodes[String(n.id)] = n;
  doc.roots.push({ root: 10, material: "default" });
  return doc;
}

/** 80·50·6 − π·8²·6 − 2·π·2.5²·6, in mm³. */
const CLOSED_FORM = 80 * 50 * 6 - Math.PI * 64 * 6 - 2 * Math.PI * 6.25 * 6;

/** Signed volume of a closed triangle mesh, by the divergence theorem. */
function meshVolume(positions: Float32Array | number[], indices: Uint32Array | number[]): number {
  let v = 0;
  for (let i = 0; i < indices.length; i += 3) {
    const a = indices[i] * 3;
    const b = indices[i + 1] * 3;
    const c = indices[i + 2] * 3;
    const [ax, ay, az] = [positions[a], positions[a + 1], positions[a + 2]];
    const [bx, by, bz] = [positions[b], positions[b + 1], positions[b + 2]];
    const [cx, cy, cz] = [positions[c], positions[c + 1], positions[c + 2]];
    v +=
      (ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx)) / 6;
  }
  return Math.abs(v);
}

describe("chained Difference in the WASM kernel", () => {
  it("evaluates through the kernel binding without trapping", () => {
    const scene = wasm.evaluateDocument(JSON.stringify(chainedDifferenceDoc()), true);
    expect(scene.parts).toHaveLength(1);
    const mesh = scene.parts[0].mesh;
    expect(mesh.indices.length).toBeGreaterThan(0);
    // The plate is 6 mm thick with 3 bores: a mesh with the cuts missing is
    // 24000 mm³, a mesh that lost the plate is far smaller.
    expect(meshVolume(mesh.positions, mesh.indices)).toBeCloseTo(CLOSED_FORM, -1);
  });

  it("does not fall back to the TypeScript evaluator", () => {
    const scene = evaluateDocument(chainedDifferenceDoc(), wasm);

    const fallbacks = warn.mock.calls.filter((c) =>
      String(c[0]).includes("falling back to TS"),
    );
    expect(
      fallbacks,
      "the kernel trapped and the engine silently re-evaluated in TypeScript",
    ).toEqual([]);

    const mesh = scene.parts[0].mesh;
    expect(meshVolume(mesh.positions, mesh.indices)).toBeCloseTo(CLOSED_FORM, -1);
  });

  /**
   * The B-rep itself, not just "a mesh came back".
   *
   * `camOutlineFromDocument` re-evaluates the document inside the kernel and
   * sections `part.solid.as_brep()` when there is topology to section —
   * reporting `mesh_source: "raw_tessellation"` — and the scene's export mesh
   * otherwise, reporting `"export_mesh"`. So the field is a direct readout of
   * whether the chain survived as a solid: a document that trapped has no
   * part at all, and one evaluated without topology says `"export_mesh"`.
   * This is also the binding that has no TypeScript fallback, and the reason
   * the bug stopped being invisible.
   */
  it("keeps a B-rep the sectioner can reach", () => {
    const outline = JSON.parse(
      wasm.camOutlineFromDocument(
        JSON.stringify(chainedDifferenceDoc()),
        0,
        3,
        false,
        "{}",
      ),
    );

    expect(outline.error, "sectioning the chained part failed").toBeUndefined();
    expect(
      outline.mesh_source,
      "the part came back without topology, so the chain lost its B-rep",
    ).toBe("raw_tessellation");

    // One outer boundary at z = 3 (mid-plate), with the bore and the two
    // holes as its islands — and they section as the circles they were
    // authored as, which is what the B-rep is worth reaching for.
    expect(outline.regions).toHaveLength(1);
    expect(outline.regions[0].holes).toHaveLength(3);

    const diameters = (outline.circles as { diameter: number }[])
      .map((c) => c.diameter)
      .sort((a, b) => a - b);
    expect(diameters).toHaveLength(3);
    expect(diameters[0]).toBeCloseTo(5, 2);
    expect(diameters[1]).toBeCloseTo(5, 2);
    expect(diameters[2]).toBeCloseTo(16, 2);
  });
});
