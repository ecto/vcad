import { describe, it, expect } from "vitest";
import {
  buildContinueTargets,
  buildSeedPrompt,
  DEFAULT_MCP_URL,
  type ContinueHost,
} from "../lib/continue-links";

const TOKEN = "11111111-2222-3333-4444-555555555555";

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
    const b64 = t.url!.split("config=")[1];
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

  it("honors a custom mcpUrl override", () => {
    const t = buildContinueTargets({
      token: TOKEN,
      mcpUrl: "https://staging.vcad.io/mcp",
    }).find((x) => x.host === "cursor")!;
    const cfg = JSON.parse(atob(t.url!.split("config=")[1]));
    expect(cfg.url).toBe("https://staging.vcad.io/mcp");
  });
});
