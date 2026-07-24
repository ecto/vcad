import { describe, it, expect, beforeAll } from "vitest";
import { getKernelWasm } from "../wasm-singleton.js";
import {
  embroideryPatternToMesh,
  embroideryPatternToMeshWithKernel,
  transformMesh,
  transformMeshWithKernel,
} from "../evaluate.js";
import type { TransformInfo } from "../transform-walk.js";
import type { EmbroideryPatternOp } from "@vcad/ir";

/**
 * Parity between the kernel-WASM mesh bindings and the TS implementations
 * they replace: `embroideryDesignToMesh` vs `embroideryPatternToMesh`, and
 * `transformMeshBuffers` vs `transformMesh`. The TS versions survive as
 * fallbacks for older WASM builds; this pins them bit-close so switching
 * paths is invisible.
 *
 * Skips when the loaded WASM artifact predates the bindings (the committed
 * artifact refreshes on main; CI's TS job builds WASM from source).
 */

/* eslint-disable @typescript-eslint/no-explicit-any */
let wasm: any;

beforeAll(async () => {
  wasm = await getKernelWasm();
});

const sampleEmbroidery = (): EmbroideryPatternOp =>
  ({
    type: "EmbroideryPattern",
    design: {
      threads: [
        { color: [255, 0, 0], name: "red" },
        { color: [0, 128, 255], name: "blue" },
      ],
      stitch_groups: [
        {
          thread_index: 0,
          stitches: [
            [0, 0],
            [10, 0],
            [10, 5],
            [10, 5], // zero-length segment — must be skipped
            [3.25, -7.5],
          ],
        },
        { thread_index: 1, stitches: [[-4, 2], [6, 9.75]] },
        { thread_index: 99, stitches: [[0, 0], [1, 1]] }, // unresolved thread → gray
      ],
      hoop_width: 100,
      hoop_height: 100,
    },
  }) as unknown as EmbroideryPatternOp;

function expectClose(a: ArrayLike<number>, b: ArrayLike<number>, tol: number) {
  expect(a.length).toBe(b.length);
  for (let i = 0; i < a.length; i++) {
    if (Math.abs(a[i] - b[i]) > tol) {
      // Use expect for a useful failure message.
      expect(a[i], `element ${i}`).toBeCloseTo(b[i], 5);
    }
  }
}

describe("kernel mesh bindings vs TS fallbacks", () => {
  it("embroideryDesignToMesh matches embroideryPatternToMesh", (ctx) => {
    if (!wasm.embroideryDesignToMesh) return ctx.skip();
    const op = sampleEmbroidery();
    const ts = embroideryPatternToMesh(op);
    const kernel = embroideryPatternToMeshWithKernel(op, wasm);

    expect(Array.from(kernel.indices)).toEqual(Array.from(ts.indices));
    expectClose(kernel.positions, ts.positions, 1e-6);
    expectClose(kernel.colors!, ts.colors!, 1e-6);
    // Sanity: the fixture actually produced geometry.
    expect(ts.positions.length).toBeGreaterThan(0);
  });

  it("transformMeshBuffers matches transformMesh (multi-axis rotation)", (ctx) => {
    if (!wasm.transformMeshBuffers) return ctx.skip();
    const mesh = {
      positions: new Float32Array([1, 0, 0, 0, 1, 0, 0, 0, 1, 2, 3, 5, -1, 4, -2, 7, -6, 1.5]),
      indices: new Uint32Array([0, 1, 2, 3, 4, 5]),
      normals: new Float32Array([0, 0, 1, 0, 1, 0, 1, 0, 0, 0.577, 0.577, 0.577, -1, 0, 0, 0, -1, 0]),
    };
    const t: TransformInfo = {
      translate: { x: 10, y: -5, z: 3 },
      rotate: { x: 30, y: 45, z: 60 },
      scale: { x: 1, y: 2, z: 0.5 },
    };
    const ts = transformMesh(mesh, t);
    const kernel = transformMeshWithKernel(mesh, t, wasm);

    expect(kernel.indices).toBe(mesh.indices);
    expectClose(kernel.positions, ts.positions, 1e-5);
    expectClose(kernel.normals!, ts.normals!, 1e-5);
  });

  it("transformMeshBuffers without normals returns no normals", (ctx) => {
    if (!wasm.transformMeshBuffers) return ctx.skip();
    const mesh = {
      positions: new Float32Array([1, 2, 3]),
      indices: new Uint32Array([]),
    };
    const t: TransformInfo = {
      translate: { x: 1, y: 2, z: 3 },
      rotate: { x: 0, y: 0, z: 90 },
      scale: { x: 1, y: 1, z: 1 },
    };
    const kernel = transformMeshWithKernel(mesh, t, wasm);
    expect(kernel.normals).toBeUndefined();
    expectClose(kernel.positions, transformMesh(mesh, t).positions, 1e-5);
  });
});
