/**
 * Tripwire for MCP surfaces deprecated by the 2026-07-28 spec RC.
 *
 * Roots, Sampling, and Logging are deprecated (12-month removal window;
 * replacements: tool params / direct LLM APIs / stderr + OpenTelemetry).
 * vcad's server has never used any of them — these tests keep it that way
 * so an SDK bump or a well-meaning contributor can't quietly adopt a
 * surface that dies mid-2027. Elicitation is NOT deprecated — it is the
 * blessed replacement pattern and is exercised by the ordering tools.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

async function connect() {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client(
    { name: "test", version: "0.0.0" },
    { capabilities: {} },
  );
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

describe("deprecated MCP surface tripwire (roots / sampling / logging)", () => {
  it("advertises no deprecated capabilities", async () => {
    const { client, server } = await connect();
    const caps = client.getServerCapabilities() as Record<string, unknown>;
    expect(caps).toBeDefined();
    for (const key of ["sampling", "roots", "logging"]) {
      expect(caps[key], `server must not advertise \`${key}\``).toBeUndefined();
    }
    await client.close();
    await server.close();
  });

  it("registers no handlers for deprecated methods", async () => {
    const { client, server } = await connect();
    const handlers = (
      server as unknown as { _requestHandlers?: Map<string, unknown> }
    )._requestHandlers;
    expect(handlers, "SDK internal _requestHandlers map").toBeDefined();
    for (const method of [
      "logging/setLevel",
      "sampling/createMessage",
      "roots/list",
    ]) {
      expect(
        handlers?.has(method),
        `no handler may be registered for \`${method}\``,
      ).toBe(false);
    }
    await client.close();
    await server.close();
  });

  it("first-party source never calls the deprecated SDK surface", () => {
    // Static sweep of src/ (excluding generated viewer bundles, which
    // legitimately contain GLSL "sampling" strings, and this test dir).
    const srcRoot = fileURLToPath(new URL("..", import.meta.url));
    const offenders: string[] = [];
    const banned = [
      "sendLoggingMessage",
      "logging/setLevel",
      "sampling/createMessage",
      "listRoots",
      "roots/list",
    ];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir)) {
        const p = join(dir, entry);
        if (statSync(p).isDirectory()) {
          if (entry === "__tests__" || entry === "node_modules") continue;
          walk(p);
          continue;
        }
        if (!p.endsWith(".ts") || p.endsWith(".generated.ts")) continue;
        const text = readFileSync(p, "utf8");
        for (const needle of banned) {
          if (text.includes(needle)) offenders.push(`${p}: ${needle}`);
        }
      }
    };
    walk(srcRoot);
    expect(offenders, offenders.join("\n")).toEqual([]);
  });
});
