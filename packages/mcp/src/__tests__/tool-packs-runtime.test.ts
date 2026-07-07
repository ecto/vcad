import { describe, it, expect, beforeAll, beforeEach, afterEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { ToolListChangedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";
import { createServer } from "../server.js";
import { resetInMemoryPackStore } from "../session-store.js";
import type { AuthUser } from "../oauth.js";

/**
 * Runtime tool-pack switching (issue #432): `set_tool_packs` flips the exposed
 * surface at runtime. On a persistent transport (stdio) the change is live —
 * ListTools reflects it and `notifications/tools/list_changed` fires. On the
 * stateless HTTP transport a signed-in user's choice is persisted and applies
 * on the next request; here we simulate that with the in-memory pack store fake.
 */

type Json = { content: Array<{ type: string; text: string }>; isError?: boolean };

/** Drive a tool through an in-memory MCP client/server pair. Returns both the
 *  client (to list tools / observe notifications) and a call helper. */
async function connect(user: AuthUser | null) {
  const engine = await Engine.init();
  const server = await createServer(engine, { user });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "t", version: "0.0.0" }, { capabilities: {} });
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  const call = async (name: string, args: Record<string, unknown> = {}) =>
    (await client.callTool({ name, arguments: args })) as unknown as Json;
  const names = async () => (await client.listTools()).tools.map((t) => t.name);
  return { server, client, call, names };
}

describe("runtime tool packs", () => {
  beforeAll(async () => {
    await Engine.init();
  });

  beforeEach(() => {
    resetInMemoryPackStore();
    delete process.env.VCAD_MCP_PACKS;
  });

  afterEach(() => {
    delete process.env.VCAD_MCP_PACKS;
  });

  it("list_tool_packs reports every pack enabled by default with tool counts", async () => {
    const { client, call } = await connect(null);
    const out = JSON.parse((await call("list_tool_packs")).content[0].text);
    expect(out.core_always_on).toBe(true);
    const packs: Array<{ name: string; enabled: boolean; tool_count: number }> = out.packs;
    expect(packs.length).toBeGreaterThan(0);
    expect(packs.every((p) => p.enabled)).toBe(true);
    const ecad = packs.find((p) => p.name === "ecad");
    expect(ecad?.tool_count).toBeGreaterThan(0);
    await client.close();
  });

  it("stdio: set_tool_packs updates ListTools live and emits list_changed", async () => {
    const { client, call, names } = await connect(null);

    let listChangedFired = false;
    client.setNotificationHandler(
      ToolListChangedNotificationSchema,
      async () => {
        listChangedFired = true;
      },
    );

    // Baseline: an ecad tool is present.
    expect(await names()).toContain("run_drc");

    // Disable everything but dfm.
    const res = JSON.parse((await call("set_tool_packs", { set: ["dfm"] })).content[0].text);
    expect(res.enabled).toEqual(["dfm"]);
    expect(res.list_changed_sent).toBe(true);

    // Wait a tick for the notification to be delivered over the in-memory pair.
    await new Promise((r) => setTimeout(r, 10));
    expect(listChangedFired).toBe(true);

    // ListTools reflects the change immediately: dfm stays, ecad is gone, core stays.
    const after = await names();
    expect(after).toContain("dfm_check");
    expect(after).not.toContain("run_drc");
    expect(after).toContain("create_cad_loon");
    // Meta-tools are always-on core, never gated.
    expect(after).toContain("list_tool_packs");
    expect(after).toContain("set_tool_packs");

    await client.close();
  });

  it("a disabled-pack call returns an actionable error naming set_tool_packs", async () => {
    const { client, call } = await connect(null);
    await call("set_tool_packs", { set: "none" });
    const err = await call("run_drc", { document_id: "x" });
    expect(err.isError).toBe(true);
    expect(err.content[0].text).toContain("ecad");
    expect(err.content[0].text).toContain("set_tool_packs");
    await client.close();
  });

  it("enable/disable deltas compose over the current set", async () => {
    const { client, call } = await connect(null);
    await call("set_tool_packs", { set: "none" });
    await call("set_tool_packs", { enable: ["ecad", "dfm"] });
    let state = JSON.parse((await call("list_tool_packs")).content[0].text).packs;
    expect(state.find((p: { name: string }) => p.name === "ecad").enabled).toBe(true);
    expect(state.find((p: { name: string }) => p.name === "dfm").enabled).toBe(true);
    expect(state.find((p: { name: string }) => p.name === "physics").enabled).toBe(false);

    await call("set_tool_packs", { disable: ["ecad"] });
    state = JSON.parse((await call("list_tool_packs")).content[0].text).packs;
    expect(state.find((p: { name: string }) => p.name === "ecad").enabled).toBe(false);
    expect(state.find((p: { name: string }) => p.name === "dfm").enabled).toBe(true);
    await client.close();
  });

  it("rejects an unknown pack name without mutating state", async () => {
    const { client, call, names } = await connect(null);
    const before = await names();
    const res = await call("set_tool_packs", { enable: ["nope"] });
    expect(res.isError).toBe(true);
    expect(res.content[0].text).toContain("Unknown pack(s): nope");
    expect(await names()).toEqual(before);
    await client.close();
  });

  it("stateless HTTP: a signed-in user's choice persists to the next request", async () => {
    const user: AuthUser = { sub: "user-abc", email: "a@b.co" };

    // Request 1: the user trims to dfm only. A fresh server (like a new
    // stateless HTTP request) is used per connection.
    const first = await connect(user);
    const res = JSON.parse(
      (await first.call("set_tool_packs", { set: ["dfm"] })).content[0].text,
    );
    expect(res.enabled).toEqual(["dfm"]);
    await first.client.close();
    await first.server.close();

    // Request 2: a brand-new server for the same user re-derives the saved
    // preference from the (in-memory fake) durable store.
    const second = await connect(user);
    const after = await second.names();
    expect(after).toContain("dfm_check");
    expect(after).not.toContain("run_drc");
    const info = JSON.parse(
      (await second.call("server_info")).content[0].text,
    );
    expect(info.packs).toBe("dfm");
    await second.client.close();
    await second.server.close();

    // A different user is unaffected — still sees the full surface.
    const other = await connect({ sub: "user-xyz", email: "x@y.co" });
    expect(await other.names()).toContain("run_drc");
    await other.client.close();
    await other.server.close();
  });
});
