/**
 * Regression: a kernel panic (wasm32 compiles `panic!` to an `unreachable`
 * trap) must not poison the shared WASM instance for every other session.
 *
 * The kernel WASM is a single process-wide instance; before this fix, one
 * trapped call left its shadow stack and linear memory in an undefined state
 * and every subsequent kernel call across all sessions failed until the
 * server was restarted — a real availability bug, since a hosted server
 * can't be restarted by a client. The fix drops the trapped instance and
 * re-instantiates a fresh one in place (see
 * packages/engine/src/wasm-singleton.ts `resetKernelWasm`).
 *
 * `renderView` and the server-level dispatch net call `resetKernelWasm` on
 * any `WebAssembly.RuntimeError`, so a bad document fails alone.
 */

import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import {
  Engine,
  getKernelWasm,
  resetKernelWasm,
  kernelWasmGeneration,
} from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { renderView } from "../tools/render.js";
import { openDocument, documents } from "../tools/session.js";

/** The exact bug-report document: a disc built entirely from cylinders, so
 *  every boolean operand carries the IR `segments == 0` "auto" sentinel.
 *  On a kernel build predating the segment-resolution fix this traps during
 *  render; on a fixed build it renders cleanly. Either way the server must
 *  stay alive for the next document. */
const DISC =
  "[difference [union [translate 0 0 -0.5 [cylinder 4 2.6]] [circular-pattern 0 0 0 0 0 1 3 360 [translate 25 0 -0.5 [cylinder 1.6 2.6]]]] [cylinder 30 1.6]]";

function makeCubeDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "test_cube",
        op: { type: "Cube", size: { x: 10, y: 10, z: 10 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  } as unknown as Document;
}

function openWith(doc: Document): string {
  const open = openDocument({ initial: doc });
  return JSON.parse(open.content[0].text).document_id as string;
}

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

describe("kernel trap recovery", () => {
  it("keeps serving other sessions after a render trap", async () => {
    // Build the disc via loon (loon → IR only; the boolean/tessellation that
    // can trap happens later, at render time).
    const discDoc = engine.evalVcadSource(DISC);
    expect(discDoc, "loon eval should produce a document").toBeTruthy();
    const discId = openWith(discDoc as Document);

    // Render the disc. On an unfixed kernel this traps and renderView reports
    // a structured "kernel trap during render" after resetting the instance;
    // on a fixed kernel it renders. Both are acceptable — what must NOT happen
    // is the instance staying poisoned.
    const discOut = await renderView({ document_id: discId });
    if (discOut.isError) {
      const text = (discOut.content[0] as { text: string }).text;
      expect(text).toContain("kernel trap during render");
      // The recovery message must NOT tell the client to restart the server.
      expect(text).not.toContain("restart the MCP server");
    }

    // The decisive check: a *different* session renders fine afterwards. If
    // the trapped instance had poisoned the process, this would throw or fail.
    const cubeId = openWith(makeCubeDoc());
    const cubeOut = await renderView({ document_id: cubeId });
    expect(cubeOut.isError, "cube render after disc must succeed").toBeFalsy();
    const image = cubeOut.content.find((c) => c.type === "image");
    expect(image, "expected a PNG after recovery").toBeDefined();
  });

  it("resetKernelWasm re-instantiates and stable references survive", async () => {
    const mod = (await getKernelWasm()) as unknown as {
      render_svg: (json: string, scale: number) => string;
    };
    // Capture the export the way Engine captures its kernel refs at init —
    // the recovery contract is that this stays valid across a reset.
    const renderSvg = mod.render_svg;
    const cubeJson = JSON.stringify(makeCubeDoc());

    expect(renderSvg(cubeJson, 2).length).toBeGreaterThan(100);

    const genBefore = kernelWasmGeneration();
    resetKernelWasm("test-induced reset");
    // Re-instantiation may be eager (synchronous, inside resetKernelWasm when a
    // source buffer is retained — the production path) or lazy (deferred to the
    // next getKernelWasm). Force whichever applies, then assert a fresh
    // instance is live without coupling the test to that internal choice.
    await getKernelWasm();
    expect(kernelWasmGeneration()).toBeGreaterThan(genBefore);

    // Same captured reference, fresh underlying instance: still works because
    // the glue's exports read the module-level `wasm` binding at call time.
    // This is exactly the property that lets Engine's captured kernel refs
    // survive a reset.
    expect(renderSvg(cubeJson, 2).length).toBeGreaterThan(100);
  });
});
