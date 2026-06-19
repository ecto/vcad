import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  captureToolCall,
  configureTelemetry,
  flushTelemetry,
  telemetryConfig,
} from "../telemetry.js";

const KEY = "phc_test_key";

describe("captureToolCall (PostHog)", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn(async () => ({ ok: true, status: 200, statusText: "OK" }));
    vi.stubGlobal("fetch", fetchMock);
    telemetryConfig.apiKey = "";
    telemetryConfig.host = "https://us.i.posthog.com";
    configureTelemetry({
      version: "1.2.3",
      build_sha: "abc1234",
      instance_id: "inst-1",
    });
  });

  afterEach(async () => {
    await flushTelemetry();
    vi.unstubAllGlobals();
  });

  it("is a no-op when no API key is set", async () => {
    captureToolCall("inspect_cad", { document_id: "abc" }, {});
    await flushTelemetry();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("posts a mcp_tool_call event with build identity to /capture/", async () => {
    telemetryConfig.apiKey = KEY;
    captureToolCall("create", { document_id: "doc1" }, {});
    await flushTelemetry();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://us.i.posthog.com/capture/");
    const payload = JSON.parse(init.body);
    expect(payload.api_key).toBe(KEY);
    expect(payload.event).toBe("mcp_tool_call");
    expect(payload.properties.tool).toBe("create");
    expect(payload.properties.is_error).toBe(false);
    expect(payload.properties.mcp_version).toBe("1.2.3");
    expect(payload.properties.build_sha).toBe("abc1234");
    expect(payload.properties.instance_id).toBe("inst-1");
  });

  it("keys anonymous events by session and suppresses person profiles", async () => {
    telemetryConfig.apiKey = KEY;
    captureToolCall("export_cad", { document_id: "doc2", format: "stl" }, {});
    await flushTelemetry();

    const payload = JSON.parse(fetchMock.mock.calls[0][1].body);
    expect(payload.distinct_id).toBe("doc2");
    expect(payload.properties.authenticated).toBe(false);
    expect(payload.properties.$process_person_profile).toBe(false);
    expect(payload.properties.document_id).toBe("doc2");
    // No arg values beyond the session id leak into the event.
    expect(payload.properties.format).toBeUndefined();
    expect(payload.properties.$set).toBeUndefined();
  });

  it("flags the error result", async () => {
    telemetryConfig.apiKey = KEY;
    captureToolCall("run_drc", { document_id: "d" }, { isError: true });
    await flushTelemetry();
    const payload = JSON.parse(fetchMock.mock.calls[0][1].body);
    expect(payload.properties.is_error).toBe(true);
  });

  it("never throws when the capture POST fails", async () => {
    telemetryConfig.apiKey = KEY;
    fetchMock.mockRejectedValue(new Error("network down"));
    expect(() => captureToolCall("create", {}, {})).not.toThrow();
    await expect(flushTelemetry()).resolves.not.toThrow();
  });
});
