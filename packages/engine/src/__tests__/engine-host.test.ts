/**
 * The hosted-engine client against the real kernel WASM (in-process
 * transport): its delta-folded view must equal the engine's full state
 * after every command, refusals leave the view alone, and requests
 * serialize.
 */
import { beforeAll, describe, expect, it } from "vitest";
import { EngineRefusal } from "../delta.js";
import { DocumentEngineClient, inProcessTransport, type HostedEngineClass } from "../engine-host.js";
import { getKernelWasm } from "../wasm-singleton.js";

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
});
