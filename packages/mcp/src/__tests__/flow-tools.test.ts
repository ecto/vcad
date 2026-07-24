import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * Registration/schema coverage for the flow solve tool. Runtime solves are
 * intentionally not exercised here: the checked-in kernel WASM only gains
 * `simulateFlow` when the artifacts are refreshed on main, so this suite
 * verifies the tool surface and fails gracefully (typed engine error, not a
 * crash) when the binding is absent.
 */

const probeEngine = await Engine.init();
const wasmHasFlow =
  typeof (
    probeEngine as unknown as {
      kernel?: { simulateFlow?: unknown };
    }
  ).kernel?.simulateFlow === "function";

type ToolText = { content: Array<{ type: string; text: string }> };

describe("flow tool registration", () => {
  let client: Client;

  beforeAll(async () => {
    documents.clear();
    const engine = await Engine.init();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    client = new Client(
      { name: "flow-tools", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
  });

  it("simulate_flow is registered with the expected schema", async () => {
    const { tools } = await client.listTools();
    const tool = tools.find((t) => t.name === "simulate_flow");
    expect(tool).toBeDefined();
    expect(tool!.annotations?.readOnlyHint).toBe(true);
    const props = (tool!.inputSchema as { properties: Record<string, unknown> })
      .properties;
    expect(props.spec).toBeDefined();
    expect(props.options).toBeDefined();
    expect(props.include_fields).toBeDefined();
    expect(
      (tool!.inputSchema as { required?: string[] }).required,
    ).toContain("spec");
  });

  it.skipIf(wasmHasFlow)(
    "reports a clear error when the kernel WASM lacks simulateFlow",
    async () => {
      const res = (await client.callTool({
        name: "simulate_flow",
        arguments: {
          spec: {
            origin_mm: [0, 0, 0],
            size_mm: [10, 10, 10],
            divisions: [10, 10, 10],
          },
        },
      })) as ToolText & { isError?: boolean };
      expect(res.isError).toBe(true);
      expect(res.content[0].text).toContain("simulateFlow");
    },
  );
});
