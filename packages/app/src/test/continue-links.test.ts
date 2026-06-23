import { describe, it, expect } from "vitest";
import {
  buildContinueTargets,
  buildSeedPrompt,
  encodeDocForSeed,
  MAX_INLINE_BLOB,
  DEFAULT_MCP_URL,
  type ContinueHost,
} from "../lib/continue-links";

const TOKEN = "11111111-2222-3333-4444-555555555555";

/** Decode a base64url gzip blob using only Web APIs (no node deps). */
async function decodeBlob(blob: string): Promise<string> {
  const b64 = blob.replace(/-/g, "+").replace(/_/g, "/");
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  const ds = new DecompressionStream("gzip");
  const stream = new Blob([bytes]).stream().pipeThrough(ds);
  return new Response(stream).text();
}

const byHost = (host: ContinueHost) => {
  const t = buildContinueTargets({ token: TOKEN, docName: "Bracket" }).find(
    (x) => x.host === host,
  );
  if (!t) throw new Error(`no target for ${host}`);
  return t;
};

const qOf = (url: string) =>
  decodeURIComponent(url.slice(url.indexOf("q=") + 2));

describe("buildSeedPrompt", () => {
  it("carries the token + tool name + doc name, never geometry", () => {
    const seed = buildSeedPrompt(TOKEN, "Bracket");
    expect(seed).toContain(TOKEN);
    expect(seed).toContain("continue_document");
    expect(seed).toContain("Bracket");
    expect(seed.length).toBeLessThan(600); // stays well under any URL cap
  });

  it("degrades gracefully without a doc name", () => {
    expect(buildSeedPrompt(TOKEN)).toContain("my vcad part");
  });
});

describe("buildContinueTargets", () => {
  it("Claude Desktop uses the claude:// scheme and prefills the seed", () => {
    const t = byHost("claude-desktop");
    expect(t.url?.startsWith("claude://claude.ai/new?q=")).toBe(true);
    expect(qOf(t.url!)).toBe(buildSeedPrompt(TOKEN, "Bracket"));
    expect(t.copyWithOpen).toBeFalsy();
  });

  it("claude.ai web has no prefill — opens + copies the seed", () => {
    const t = byHost("claude-web");
    expect(t.url).toBe("https://claude.ai/new");
    expect(t.url).not.toContain("q=");
    expect(t.clipboard).toBe(buildSeedPrompt(TOKEN, "Bracket"));
    expect(t.copyWithOpen).toBe(true);
  });

  it("ChatGPT prefills via chatgpt.com/?q=", () => {
    const t = byHost("chatgpt");
    expect(t.url?.startsWith("https://chatgpt.com/?q=")).toBe(true);
    expect(qOf(t.url!)).toBe(buildSeedPrompt(TOKEN, "Bracket"));
  });

  it("Cursor encodes a base64 server config + name param", () => {
    const t = byHost("cursor");
    expect(
      t.url?.startsWith(
        "cursor://anysphere.cursor-deeplink/mcp/install?name=vcad&config=",
      ),
    ).toBe(true);
    const b64 = t.url!.split("config=")[1]!;
    const cfg = JSON.parse(atob(b64));
    expect(cfg).toEqual({ type: "http", url: DEFAULT_MCP_URL });
  });

  it("VS Code URL-encodes JSON with name inline (not base64)", () => {
    const t = byHost("vscode");
    expect(t.url?.startsWith("vscode:mcp/install?")).toBe(true);
    const cfg = JSON.parse(
      decodeURIComponent(t.url!.slice("vscode:mcp/install?".length)),
    );
    expect(cfg).toEqual({ name: "vcad", type: "http", url: DEFAULT_MCP_URL });
  });

  it("Claude Code is a copyable install command + seed (no url)", () => {
    const t = byHost("claude-code");
    expect(t.url).toBeUndefined();
    expect(t.clipboard).toContain("claude mcp add --transport http vcad");
    expect(t.clipboard).toContain(DEFAULT_MCP_URL);
    expect(t.clipboard).toContain(TOKEN);
  });

  it("inlineDoc builds a seed embedding the blob, with no token", () => {
    const t = buildContinueTargets({
      inlineDoc: "BLOB123",
      docName: "Bracket",
    }).find((x) => x.host === "claude-desktop")!;
    const seed = decodeURIComponent(t.url!.slice(t.url!.indexOf("q=") + 2));
    expect(seed).toContain('doc="BLOB123"');
    expect(seed).not.toContain(TOKEN);
  });

  it("honors a custom mcpUrl override", () => {
    const t = buildContinueTargets({
      token: TOKEN,
      mcpUrl: "https://staging.vcad.io/mcp",
    }).find((x) => x.host === "cursor")!;
    const cfg = JSON.parse(atob(t.url!.split("config=")[1]!));
    expect(cfg.url).toBe("https://staging.vcad.io/mcp");
  });
});

describe("encodeDocForSeed", () => {
  it("round-trips an IR doc through gzip + base64url", async () => {
    const doc = { version: 1, nodes: {}, roots: ["n1"] };
    const blob = await encodeDocForSeed(doc);
    expect(blob).toBeTruthy();
    expect(blob).toMatch(/^[A-Za-z0-9_-]+$/); // base64url, no padding
    expect(JSON.parse(await decodeBlob(blob!))).toEqual(doc);
  });

  it("returns null when the part is too large to inline", async () => {
    // A doc whose compressed form blows past MAX_INLINE_BLOB. Random-ish keys
    // resist gzip so the blob actually exceeds the cap.
    const nodes: Record<string, unknown> = {};
    for (let i = 0; i < 4000; i++) {
      nodes[`node_${i}_${(i * 2654435761) % 1e9}`] = {
        op: "cube",
        size: [i % 97, (i * 7) % 89, (i * 13) % 83],
      };
    }
    const blob = await encodeDocForSeed({ version: 1, nodes, roots: [] });
    expect(blob).toBeNull();
    expect(MAX_INLINE_BLOB).toBeGreaterThan(0);
  });
});
