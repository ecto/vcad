/**
 * Store-as-view, PR 2: actions moved onto the store's `DocumentEngineClient`
 * fold engine deltas into the store, and interleave safely with actions that
 * still call the engine synchronously (the client resyncs on a hash miss).
 */
import { beforeAll, describe, expect, it } from "vitest";
import { getKernelWasm } from "@vcad/engine";
import { useDocumentStore, type WasmDocumentEngineConstructor } from "../stores/document-store.js";

let Engine: WasmDocumentEngineConstructor | undefined;
beforeAll(async () => {
  const wasm = (await getKernelWasm()) as unknown as { WasmDocumentEngine?: WasmDocumentEngineConstructor };
  Engine = wasm.WasmDocumentEngine;
});

const hasDeltas = () =>
  typeof (Engine?.prototype as { applyDelta?: unknown } | undefined)?.applyDelta === "function";

describe("document store dispatch", () => {
  it("async actions fold deltas and interleave with sync ones", async () => {
    if (!hasDeltas()) {
      // Checked-in artifacts can predate deltas; CI builds from source.
      if (process.env.CI) throw new Error("kernel WASM lacks applyDelta");
      return;
    }
    const s = useDocumentStore.getState();
    s._initCrdt(Engine!);
    const a = s.addPrimitive("cube");
    const b = s.addPrimitive("sphere");
    await useDocumentStore.getState().renamePart(a, "Renamed");
    // A sync action moves the engine behind the client's view…
    useDocumentStore.getState().setTranslation(b, { x: 4, y: 0, z: 0 });
    // …and the next async action still lands on the current state.
    await useDocumentStore.getState().setPartVisible(b, false);
    await useDocumentStore.getState().removePart(a);

    const st = useDocumentStore.getState();
    const engine = st._crdtEngine!;
    expect(st.document).toEqual(JSON.parse(engine.get_document_json()));
    expect(st.parts.map((p) => p.id)).toEqual([b]);
    expect(st.isDirty).toBe(true);
  });

  it("undo pressed while a command is in flight undoes that command", async () => {
    if (!hasDeltas()) {
      if (process.env.CI) throw new Error("kernel WASM lacks applyDelta");
      return;
    }
    const s = useDocumentStore.getState();
    const id = s.addPrimitive("cube");
    // Not awaited: the rename is still queued when undo is pressed.
    const rename = useDocumentStore.getState().renamePart(id, "Gone");
    const undo = useDocumentStore.getState().undo();
    await Promise.all([rename, undo]);
    const st = useDocumentStore.getState();
    expect(st.document).toEqual(JSON.parse(st._crdtEngine!.get_document_json()));
    expect(JSON.stringify(st.document)).not.toContain('"Gone"');
    expect(st.parts.map((p) => p.id)).toContain(id);
    // Undo with nothing left is a quiet no-op, not a rejection.
    while (st._crdtEngine!.can_undo()) await useDocumentStore.getState().undo();
    await expect(useDocumentStore.getState().undo()).resolves.toBeUndefined();
  });
});
