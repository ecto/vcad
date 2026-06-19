import {
  describe,
  it,
  expect,
  beforeEach,
  afterEach,
  vi,
} from "vitest";
import { fireToolAlert, fireSessionAlert, notifyConfig } from "../notify.js";

const WEBHOOK = "https://discord.com/api/webhooks/test/token";
// Tests run with a short rollup window so a single timer tick fires it.
const INTERVAL_MS = 60_000;

describe("fireToolAlert (rollup)", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    fetchMock = vi.fn(async () => ({ ok: true, status: 204, statusText: "" }));
    vi.stubGlobal("fetch", fetchMock);
    notifyConfig.webhookUrl = "";
    notifyConfig.rollupMs = INTERVAL_MS;
  });

  afterEach(async () => {
    // Drain any open window and let the follow-up idle tick clear the timer,
    // so module-global state doesn't leak into the next test.
    notifyConfig.webhookUrl = WEBHOOK;
    await vi.advanceTimersByTimeAsync(INTERVAL_MS * 2);
    notifyConfig.webhookUrl = "";
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("is a no-op when no webhook URL is set", async () => {
    fireToolAlert("inspect_cad", { document_id: "abc" }, {});
    await vi.advanceTimersByTimeAsync(INTERVAL_MS);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("posts an aggregated rollup after the interval", async () => {
    notifyConfig.webhookUrl = WEBHOOK;
    // 5 calls across 2 sessions, one failure.
    fireToolAlert("create", { document_id: "doc1" }, {});
    fireToolAlert("create", { document_id: "doc1" }, {});
    fireToolAlert("export_cad", { document_id: "doc1", format: "stl" }, {});
    fireToolAlert("inspect_cad", { document_id: "doc2" }, {});
    fireToolAlert("inspect_cad", { document_id: "doc2" }, { isError: true });

    // Nothing fires before the interval elapses.
    expect(fetchMock).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(INTERVAL_MS);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(WEBHOOK);
    const payload = JSON.parse(init.body);
    expect(payload.username).toBe("vcad mcp");
    expect(payload.content).toContain("vcad activity");
    expect(payload.content).toContain("5 tool calls");
    expect(payload.content).toContain("across 2 sessions");
    expect(payload.content).toContain("1 error");
    // Per-tool counts, ranked by frequency.
    expect(payload.content).toContain("`create` ×2");
    expect(payload.content).toContain("`inspect_cad` ×2");
    expect(payload.content).toContain("`export_cad` ×1");
    expect(payload.allowed_mentions).toEqual({ parse: [] });
  });

  it("stays quiet on an idle window after reporting", async () => {
    notifyConfig.webhookUrl = WEBHOOK;
    fireToolAlert("create", { document_id: "doc1" }, {});
    await vi.advanceTimersByTimeAsync(INTERVAL_MS);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    // No calls in the next window — no second message.
    await vi.advanceTimersByTimeAsync(INTERVAL_MS);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("never throws when the webhook POST fails", async () => {
    notifyConfig.webhookUrl = WEBHOOK;
    fetchMock.mockRejectedValue(new Error("network down"));
    expect(() => fireToolAlert("create", {}, {})).not.toThrow();
    await expect(
      vi.advanceTimersByTimeAsync(INTERVAL_MS),
    ).resolves.not.toThrow();
  });
});

describe("fireSessionAlert (new-session ping)", () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  const ENVS = [
    "VERCEL_ENV",
    "DISCORD_FORCE",
    "DISCORD_WEBHOOK_URL",
    "DISCORD_WEBHOOK_URL_SESSION",
    "VCAD_BUILD_SHA",
  ];
  const saved: Record<string, string | undefined> = {};

  beforeEach(() => {
    fetchMock = vi.fn(async () => ({ ok: true, status: 204, statusText: "" }));
    vi.stubGlobal("fetch", fetchMock);
    for (const k of ENVS) {
      saved[k] = process.env[k];
      delete process.env[k];
    }
    notifyConfig.webhookUrl = "";
  });

  afterEach(() => {
    for (const k of ENVS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
    vi.unstubAllGlobals();
  });

  it("no-ops when no webhook is configured (even if forced)", async () => {
    process.env.DISCORD_FORCE = "1";
    await fireSessionAlert("doc_1", "open_document", null);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("no-ops outside production unless forced", async () => {
    process.env.DISCORD_WEBHOOK_URL = WEBHOOK;
    process.env.VERCEL_ENV = "preview";
    await fireSessionAlert("doc_1", "open_document", null);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("posts an embed for a new session (forced), masking the caller", async () => {
    process.env.DISCORD_WEBHOOK_URL = WEBHOOK;
    process.env.DISCORD_FORCE = "1";
    process.env.VCAD_BUILD_SHA = "abcdef1234";
    await fireSessionAlert("doc_xyz", "create_cad_loon", {
      email: "alex@example.com",
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(WEBHOOK);
    const payload = JSON.parse(init.body);
    const embed = payload.embeds[0];
    expect(embed.title).toContain("New MCP session");
    const fields = Object.fromEntries(
      embed.fields.map((f: { name: string; value: string }) => [f.name, f.value]),
    );
    expect(fields.session).toContain("doc_xyz");
    expect(fields.via).toContain("create_cad_loon");
    expect(fields.who).toBe("a***@example.com");
    expect(fields.build).toContain("abcdef1");
    expect(payload.allowed_mentions).toEqual({ parse: [] });
  });

  it("labels an anonymous caller as 'anonymous'", async () => {
    process.env.DISCORD_WEBHOOK_URL = WEBHOOK;
    process.env.DISCORD_FORCE = "1";
    await fireSessionAlert("doc_1", "open_document", null);
    const embed = JSON.parse(fetchMock.mock.calls[0][1].body).embeds[0];
    const who = embed.fields.find(
      (f: { name: string }) => f.name === "who",
    ).value;
    expect(who).toBe("anonymous");
  });

  it("prefers the session-specific webhook override", async () => {
    process.env.DISCORD_WEBHOOK_URL = WEBHOOK;
    process.env.DISCORD_WEBHOOK_URL_SESSION = "https://discord.com/api/webhooks/sess/tok";
    process.env.DISCORD_FORCE = "1";
    await fireSessionAlert("doc_1", "open_document", null);
    expect(fetchMock.mock.calls[0][0]).toBe(
      "https://discord.com/api/webhooks/sess/tok",
    );
  });

  it("never throws when the webhook POST fails", async () => {
    process.env.DISCORD_WEBHOOK_URL = WEBHOOK;
    process.env.DISCORD_FORCE = "1";
    fetchMock.mockRejectedValue(new Error("network down"));
    await expect(
      fireSessionAlert("doc_1", "open_document", null),
    ).resolves.toBeUndefined();
  });
});
