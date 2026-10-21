/**
 * The hosted-engine client against the real kernel WASM (in-process
 * transport): its delta-folded view must equal the engine's full state
 * after every command, refusals leave the view alone, and requests
 * serialize.
 */
import { beforeAll, describe, expect, it } from "vitest";
import { EngineRefusal } from "../delta.js";
import {
  DocumentEngineClient,
  inProcessTransport,
  sharedEngineTransport,
  workerTransport,
  type HostedEngineClass,
} from "../engine-host.js";
import { inlineEngineWorker } from "../testing/inline-worker.js";
import { getKernelWasm } from "../wasm-singleton.js";
import { wasmResultToScene, type WasmEvaluatedScene } from "../scene-convert.js";

let Engine: HostedEngineClass | undefined;
beforeAll(async () => {
  const wasm = (await getKernelWasm()) as unknown as { WasmDocumentEngine?: HostedEngineClass };
  Engine = wasm.WasmDocumentEngine;
});

const hasDeltas = () => typeof (Engine?.prototype as { applyDelta?: unknown } | undefined)?.applyDelta === "function";

describe("DocumentEngineClient (in-process)", () => {
  it("folds deltas into a view that matches the engine", async () => {
    if (!hasDeltas()) {
      // Checked-in artifacts can predate deltas; CI builds from source.
      if (process.env.CI) throw new Error("kernel WASM lacks applyDelta");
      return;
    }
    const client = new DocumentEngineClient(inProcessTransport(Engine!));
    await client.init(7);
    const cube = await client.apply({
      type: "add_feature",
      name: "Box",
      feature: { type: "Cube", size_x: 10, size_y: 10, size_z: 10 },
    });
    const id = cube.createdFeatureId!;
    expect(cube.changes.rebuild).toEqual([id]);
    // Fire several without awaiting: they must serialize.
    const pending = [
      client.apply({ type: "add_feature", feature: { type: "Sphere", radius: 3 } }),
      client.apply({ type: "set_translation", id, offset: [5, 0, 0] }),
      client.applyTool("create", { type: "fillet", parent_part_id: id, params: { radius: 1 } }),
    ];
    const outcomes = await Promise.all(pending);
    expect(outcomes[1]!.changes.rebuild).toEqual([id]);
    await expect(client.apply({ type: "delete_feature", id: "9:9" })).rejects.toBeInstanceOf(EngineRefusal);
    await client.apply({ type: "undo" });
    expect(client.canRedo).toBe(true);

    const folded = client.state!;
    const full = await client.refresh();
    expect(folded).toEqual(full);

    // A snapshot round-trips through load.
    const bytes = await client.save();
    const other = new DocumentEngineClient(inProcessTransport(Engine!));
    expect(await other.load(bytes)).toEqual(full);
    expect(other.stateHash).toBe(client.stateHash);
    client.dispose();
    other.dispose();
  });

  it("evaluates in the host, incrementally, equal to a cold evaluation", async () => {
    const wasm = (await getKernelWasm()) as unknown as {
      IncrementalEvaluator?: new () => object;
      evaluateDocument: (json: string, skip: boolean) => unknown;
    };
    const canEval = typeof (Engine?.prototype as { evaluateIncremental?: unknown } | undefined)
      ?.evaluateIncremental === "function";
    if (!hasDeltas() || !canEval || !wasm.IncrementalEvaluator) {
      if (process.env.CI) throw new Error("kernel WASM lacks evaluateIncremental");
      return;
    }
    const client = new DocumentEngineClient(inProcessTransport(Engine!, wasm.IncrementalEvaluator));
    await client.init(3);
    const box = await client.apply({
      type: "add_feature",
      feature: { type: "Cube", size_x: 10, size_y: 10, size_z: 10 },
    });
    await client.apply({ type: "add_feature", feature: { type: "Sphere", radius: 4 } });
    const cold = () =>
      wasmResultToScene(
        wasm.evaluateDocument(JSON.stringify(client.state!.document), true) as WasmEvaluatedScene,
      );
    const meshes = (s: { parts: { mesh: { positions: Float32Array } }[] }) =>
      s.parts.map((p) => Array.from(p.mesh.positions));

    const first = await client.evaluate({ skipClashDetection: true });
    expect(meshes(first)).toEqual(meshes(cold()));
    // Commands then an evaluate: the scene is of the post-command state.
    const moved = client.apply({ type: "set_translation", id: box.createdFeatureId!, offset: [3, 0, 0] });
    const second = await client.evaluate({ skipClashDetection: true });
    await moved;
    expect(meshes(second)).toEqual(meshes(cold()));
    expect(first.parts).toHaveLength(2);
    client.dispose();
  });

  it("across a worker boundary: undo lands after in-flight commands", async () => {
    if (!hasDeltas()) {
      if (process.env.CI) throw new Error("kernel WASM lacks applyDelta");
      return;
    }
    const worker = inlineEngineWorker({ Engine: Engine!, engine: null });
    const client = new DocumentEngineClient(workerTransport(worker as unknown as Worker));
    await client.init(5);
    // Fire-and-forget, as UI handlers do: add, add, undo.
    const a = client.apply({ type: "add_feature", feature: { type: "Sphere", radius: 1 } });
    const b = client.apply({ type: "add_feature", feature: { type: "Sphere", radius: 2 } });
    const u = client.apply({ type: "undo" });
    const [ra, rb] = await Promise.all([a, b, u]);
    const ids = client.state!.parts.map((p) => (p as { id: string }).id);
    expect(ids).toEqual([ra!.createdFeatureId]);
    expect(ids).not.toContain(rb!.createdFeatureId);
    expect(client.canRedo).toBe(true);
    // The delta-folded view equals the worker's full state.
    const folded = client.state;
    expect(folded).toEqual(await client.refresh());
    expect(worker.handled).toBeGreaterThanOrEqual(5);
    client.dispose();
  });

  it("resyncs a shared engine's view after direct mutations", async () => {
    if (!hasDeltas()) {
      if (process.env.CI) throw new Error("kernel WASM lacks applyDelta");
      return;
    }
    const engine = new Engine!();
    const client = new DocumentEngineClient(sharedEngineTransport(() => engine));
    await client.init();
    const a = await client.apply({ type: "add_feature", feature: { type: "Sphere", radius: 2 } });
    // The owner mutates behind the client's back (the store's sync path).
    engine.applyDelta(
      JSON.stringify({ type: "add_feature", feature: { type: "Cube", size_x: 1, size_y: 1, size_z: 1 } }),
    );
    await client.apply({ type: "set_translation", id: a.createdFeatureId!, offset: [1, 2, 3] });
    expect(client.state!.parts).toHaveLength(2);
    expect(client.state).toEqual(await client.refresh());
    client.dispose(); // must not free the owner's engine
    expect(engine.stateHash()).toBe(client.stateHash);
    engine.free();
  });
});
