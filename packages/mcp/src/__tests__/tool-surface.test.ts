import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * The advertised MCP tool surface must be byte-identical across the ToolDef
 * registry refactor (issue #430) and every subsequent change. The fixture
 * `tool-surface.fixture.json` snapshots the `tools/list` result; this test
 * drives the real ListTools handler through an in-memory MCP client and asserts
 * the assembled surface (names, order, titles, descriptions, inputSchemas,
 * annotations, outputSchemas, and viewer `_meta`) still equals it.
 *
 * Regenerate the fixture ONLY on a deliberate, reviewed surface change, by
 * running the suite with `UPDATE_TOOL_SURFACE=1` — that rewrites the committed
 * fixture from the live surface instead of asserting against it.
 */

describe("tool surface is unchanged (ToolDef assembly === snapshot)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("tools/list equals the committed fixture", async () => {
    documents.clear();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "surface", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
    const { tools } = await client.listTools();

    const fixturePath = resolve(__dirname, "tool-surface.fixture.json");
    if (process.env.UPDATE_TOOL_SURFACE) {
      writeFileSync(fixturePath, JSON.stringify(tools, null, 2) + "\n");
    }
    const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));
    expect(tools).toEqual(fixture);

    await client.close();
    await server.close();
  });
});
