import {
  describe,
  it,
  expect,
  beforeEach,
  afterEach,
  vi,
} from "vitest";
import { fireToolAlert, notifyConfig } from "../notify.js";
import { telemetryConfig } from "../telemetry.js";

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
    // Keep the PostHog sink off so fetch-call counts reflect only Discord,
    // regardless of any POSTHOG_API_KEY in the ambient environment.
    telemetryConfig.apiKey = "";
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
