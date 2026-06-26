import { describe, it, expect, beforeEach, afterEach, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer, getBuildInfo } from "../server.js";
import {
  computeStaleness,
  getExpectedBuildSha,
  getRuntimeFlag,
  getStaleness,
  setEdgeConfigFetch,
  resetEdgeConfigCache,
} from "../edge-config.js";

/**
 * Health / observability coverage for the warm-instance-staleness fixes:
 *
 *   1. Every tool result carries the running build identity in `_meta`
 *      (build_sha + instance_id + is_stale), not just `server_info` — so version
 *      skew / a stale warm lambda is visible inline on any call.
 *   2. `server_info` reports `expected_build_sha` + `is_stale`.
 *   3. Runtime flags + the expected sha are read from Edge Config (with an env
 *      fallback) so a warm instance reflects a flip WITHOUT a redeploy.
 */

const BUILD_META_KEY = "io.vcad/build";

interface BuildMeta {
  build_sha: string;
  instance_id: string;
  version_full: string;
  uptime_s: number;
  expected_build_sha: string | null;
  is_stale: boolean;
}

function buildMetaOf(result: unknown): BuildMeta {
  const meta = (result as { _meta?: Record<string, unknown> })._meta;
  expect(meta, "result should carry _meta").toBeDefined();
  return meta![BUILD_META_KEY] as BuildMeta;
}

function firstText(result: unknown): string {
  return (result as { content: Array<{ type: string; text: string }> }).content[0]
    .text;
}

// ── Edge Config fake — returns a fixed item map for the `/items` endpoint. ──
const EDGE_CONN = "https://edge-config.vercel.com/ecfg_test?token=tok_test";
function installEdgeConfig(items: Record<string, unknown>): void {
  process.env.EDGE_CONFIG = EDGE_CONN;
  resetEdgeConfigCache();
  setEdgeConfigFetch((async () =>
    new Response(JSON.stringify(items), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    })) as unknown as typeof fetch);
}

describe("computeStaleness", () => {
  it("is never stale when the expected sha is unknown/unset", () => {
    expect(computeStaleness("abc1234def", null).is_stale).toBe(false);
    expect(computeStaleness("abc1234def", "unknown").is_stale).toBe(false);
    expect(computeStaleness("unknown", "abc1234def").is_stale).toBe(false);
  });

  it("is not stale when running matches expected (incl. short vs full sha)", () => {
    expect(computeStaleness("abc1234def", "abc1234def").is_stale).toBe(false);
    expect(computeStaleness("abc1234def0000", "abc1234").is_stale).toBe(false);
    expect(computeStaleness("abc1234", "abc1234def0000").is_stale).toBe(false);
  });

  it("is stale only when both are known and differ", () => {
    const s = computeStaleness("aaaa111", "bbbb222");
    expect(s.is_stale).toBe(true);
    expect(s.expected_build_sha).toBe("bbbb222");
  });
});

describe("edge-config runtime reads", () => {
  let savedEdge: string | undefined;
  let savedFlag: string | undefined;
  let savedExpected: string | undefined;

  beforeEach(() => {
    savedEdge = process.env.EDGE_CONFIG;
    savedFlag = process.env.VCAD_LIVE_WINDOW;
    savedExpected = process.env.VCAD_EXPECTED_BUILD_SHA;
  });

  afterEach(() => {
    for (const [k, v] of [
      ["EDGE_CONFIG", savedEdge],
      ["VCAD_LIVE_WINDOW", savedFlag],
      ["VCAD_EXPECTED_BUILD_SHA", savedExpected],
    ] as const) {
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
    }
    resetEdgeConfigCache();
    setEdgeConfigFetch(((...a: Parameters<typeof fetch>) =>
      fetch(...a)) as typeof fetch);
  });

  it("reads a feature flag from Edge Config, overriding the env var", async () => {
    delete process.env.VCAD_LIVE_WINDOW; // env says off…
    installEdgeConfig({ "flag.VCAD_LIVE_WINDOW": true }); // …Edge Config says on
    expect(await getRuntimeFlag("VCAD_LIVE_WINDOW")).toBe(true);
  });

  it("Edge Config OFF wins over an env var that is ON", async () => {
    process.env.VCAD_LIVE_WINDOW = "1";
    installEdgeConfig({ VCAD_LIVE_WINDOW: false });
    expect(await getRuntimeFlag("VCAD_LIVE_WINDOW")).toBe(false);
  });

  it("falls back to the env var when Edge Config is unset", async () => {
    delete process.env.EDGE_CONFIG;
    resetEdgeConfigCache();
    process.env.VCAD_LIVE_WINDOW = "1";
    expect(await getRuntimeFlag("VCAD_LIVE_WINDOW")).toBe(true);
    process.env.VCAD_LIVE_WINDOW = "0";
    expect(await getRuntimeFlag("VCAD_LIVE_WINDOW")).toBe(false);
  });

  it("reports staleness from the Edge Config expected_build_sha", async () => {
    installEdgeConfig({ expected_build_sha: "newsha9" });
    expect(await getExpectedBuildSha()).toBe("newsha9");
    expect((await getStaleness("oldsha1")).is_stale).toBe(true);
    expect((await getStaleness("newsha9")).is_stale).toBe(false);
  });

  it("serves the env expected sha when Edge Config is unset", async () => {
    delete process.env.EDGE_CONFIG;
    resetEdgeConfigCache();
    process.env.VCAD_EXPECTED_BUILD_SHA = "envsha7";
    expect(await getExpectedBuildSha()).toBe("envsha7");
  });
});

describe("tool results carry build identity in _meta (end-to-end)", () => {
  let engine: Engine;
  let savedEdge: string | undefined;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    savedEdge = process.env.EDGE_CONFIG;
    // Force env-only mode so the running build is never flagged stale by a
    // leaked Edge Config — keeps the _meta assertions deterministic.
    delete process.env.EDGE_CONFIG;
    resetEdgeConfigCache();
  });

  afterEach(() => {
    if (savedEdge === undefined) delete process.env.EDGE_CONFIG;
    else process.env.EDGE_CONFIG = savedEdge;
    resetEdgeConfigCache();
  });

  async function connect() {
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "test", version: "0.0.0" }, { capabilities: {} });
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
    return { client, server };
  }

  it("stamps build identity on a successful tool result", async () => {
    const { client, server } = await connect();
    const open = await client.callTool({ name: "open_document", arguments: {} });

    const meta = buildMetaOf(open);
    const info = getBuildInfo();
    expect(meta.build_sha).toBe(info.build_sha);
    expect(meta.instance_id).toBe(info.instance_id);
    expect(meta.version_full).toBe(info.version_full);
    expect(typeof meta.uptime_s).toBe("number");
    expect(typeof meta.is_stale).toBe("boolean");

    await client.close();
    await server.close();
  });

  it("stamps build identity even on an error result (unknown tool)", async () => {
    const { client, server } = await connect();
    const res = await client.callTool({
      name: "does_not_exist",
      arguments: {},
    });
    expect(res.isError).toBe(true);
    const meta = buildMetaOf(res);
    expect(meta.build_sha).toBe(getBuildInfo().build_sha);
    expect(meta.instance_id).toBe(getBuildInfo().instance_id);

    await client.close();
    await server.close();
  });

  it("server_info reports expected_build_sha + is_stale", async () => {
    const { client, server } = await connect();
    const res = await client.callTool({ name: "server_info", arguments: {} });
    const payload = JSON.parse(firstText(res)) as Record<string, unknown>;

    expect(payload).toHaveProperty("build_sha");
    expect(payload).toHaveProperty("instance_id");
    expect(payload).toHaveProperty("expected_build_sha");
    expect(payload).toHaveProperty("is_stale");
    expect(typeof payload.is_stale).toBe("boolean");
    // And the same identity rides on _meta as on every other call.
    expect(buildMetaOf(res).build_sha).toBe(getBuildInfo().build_sha);

    await client.close();
    await server.close();
  });
});
