import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";

/**
 * Locks in the viewer `_meta` split (the "one live canvas, not 100 iframes"
 * fix): only MOUNT tools carry the UI template; data tools return
 * structuredContent (document_id + document_version) and no template, so the
 * host never spawns a fresh iframe per mutation. App-only fetchers stay hidden
 * from the model. Drives the real ListTools/CallTool handlers end-to-end.
 */

const VIEWER_URI = "ui://vcad/viewer";

interface ToolMeta {
  ui?: { resourceUri?: string; visibility?: string[] };
  "openai/outputTemplate"?: string;
  "openai/widgetAccessible"?: boolean;
}
interface ToolDesc {
  name: string;
  _meta?: ToolMeta;
}

async function connect(engine: Engine) {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "test", version: "0.0.0" }, { capabilities: {} });
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

function hasTemplate(m: ToolMeta | undefined): boolean {
  return Boolean(m?.ui?.resourceUri || m?.["openai/outputTemplate"]);
}

describe("viewer _meta split (one canvas, not one iframe per call)", () => {
  let engine: Engine;
  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("mounts the template ONLY on session openers, not on data tools", async () => {
    documents.clear();
    const { client, server } = await connect(engine);
    const { tools } = (await client.listTools()) as { tools: ToolDesc[] };
    const by = (n: string) => tools.find((t) => t.name === n);

    // Mount tools: carry the UI template in both dialects.
    for (const name of [
      "open_document",
      "place_components",
      "build_receipt",
      "create_robot_env",
      "quote_manufacturing",
    ]) {
      const m = by(name)?._meta;
      expect(m?.ui?.resourceUri, `${name} ui.resourceUri`).toBe(VIEWER_URI);
      expect(m?.["openai/outputTemplate"], `${name} outputTemplate`).toBeTruthy();
    }

    // Data tools (frequent mutators): NO template — the canvas self-refreshes.
    for (const name of ["place_part", "route_nets", "add_via"]) {
      expect(hasTemplate(by(name)?._meta), `${name} must not mount`).toBe(false);
    }

    await client.close();
    await server.close();
  });

  it("keeps preview fetchers app-only (hidden from the model, no template)", async () => {
    documents.clear();
    const { client, server } = await connect(engine);
    const { tools } = (await client.listTools()) as { tools: ToolDesc[] };
    const by = (n: string) => tools.find((t) => t.name === n);

    for (const name of [
      "get_preview_glb",
      "get_preview_version",
      "get_sim_replay",
      "get_sim_version",
      "get_order_feed",
    ]) {
      const m = by(name)?._meta;
      expect(by(name), `${name} exists`).toBeDefined();
      expect(m?.ui?.visibility, `${name} app-only`).toEqual(["app"]);
      expect(m?.["openai/widgetAccessible"], `${name} widgetAccessible`).toBe(true);
      expect(hasTemplate(m), `${name} carries no template`).toBe(false);
    }

    // Widget-callable readers stay reachable from the iframe but don't mount.
    const getDoc = by("get_document")?._meta;
    expect(getDoc?.["openai/widgetAccessible"]).toBe(true);
    expect(hasTemplate(getDoc)).toBe(false);

    await client.close();
    await server.close();
  });

  it("money tools are never widget-callable (the iframe is read-only for money)", async () => {
    documents.clear();
    const { client, server } = await connect(engine);
    const { tools } = (await client.listTools()) as { tools: ToolDesc[] };
    const by = (n: string) => tools.find((t) => t.name === n);

    // The asymmetric seam: the agent proposes, the human approves out-of-band,
    // the agent places. The widget must not be able to call either money tool —
    // no template, no openai/widgetAccessible.
    for (const name of ["authorize_spend", "place_order"]) {
      const m = by(name)?._meta;
      expect(by(name), `${name} exists`).toBeDefined();
      expect(hasTemplate(m), `${name} must not mount`).toBe(false);
      expect(
        m?.["openai/widgetAccessible"],
        `${name} must not be widget-callable`,
      ).not.toBe(true);
    }

    await client.close();
    await server.close();
  });

  it("data-tool results carry a document_version token for self-refresh", async () => {
    documents.clear();
    const { client, server } = await connect(engine);

    const open = await client.callTool({ name: "open_document", arguments: {} });
    const sc = (open as { structuredContent?: Record<string, unknown> })
      .structuredContent;
    expect(typeof sc?.document_id).toBe("string");
    expect(typeof sc?.document_version).toBe("string");

    await client.close();
    await server.close();
  });

  it("empty doc: preview is a soft no-geometry result, not an error, and still has a version", async () => {
    documents.clear();
    const { client, server } = await connect(engine);
    const open = await client.callTool({ name: "open_document", arguments: {} });
    const docId = (open as { structuredContent?: Record<string, unknown> })
      .structuredContent?.document_id as string;

    // Soft: the poll loop relies on this NOT throwing for a freshly-opened
    // empty document (the build-after-open flow), so it doesn't inflate errors.
    const glb = await client.callTool({
      name: "get_preview_glb",
      arguments: { document_id: docId },
    });
    expect(glb.isError ?? false).toBe(false);
    const glbText = (glb as { content: Array<{ type: string; text: string }> })
      .content[0].text;
    expect(JSON.parse(glbText)._vcad_glb).toBeNull();

    // A version token exists even with no geometry, so a later build flips it.
    const ver = await client.callTool({
      name: "get_preview_version",
      arguments: { document_id: docId },
    });
    const verText = (ver as { content: Array<{ type: string; text: string }> })
      .content[0].text;
    expect(typeof JSON.parse(verText).version).toBe("string");

    await client.close();
    await server.close();
  });
});
