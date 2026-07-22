import { describe, expect, it } from "vitest";
import { getSolver } from "../solver.js";
import {
  DEFAULT_GATEWAY_DIRECT,
  gatewayIdSegment,
  makeGatewayDirectSolver,
} from "../solvers/gateway-direct.js";
import type { Task } from "../task.js";

const dummyTask: Task = {
  id: "a1-cube-01",
  suite: "A",
  tier: "A1",
  title: "Centered cube",
  prompt: "Make a 25mm cube centered on the origin.",
  checks: [],
};

describe("makeGatewayDirectSolver", () => {
  it("defaults to anthropic/claude-sonnet-4.5", () => {
    expect(DEFAULT_GATEWAY_DIRECT.model).toBe("anthropic/claude-sonnet-4.5");
    const s = makeGatewayDirectSolver();
    expect(s.id).toBe("gateway-direct-anthropic-claude-sonnet-4.5");
    expect(s.provider).toBe("anthropic");
  });

  it("folds the gateway slug's slash out of the solver id", () => {
    expect(gatewayIdSegment("xai/grok-4")).toBe("xai-grok-4");
    const s = makeGatewayDirectSolver({ model: "xai/grok-4" });
    expect(s.id).toBe("gateway-direct-xai-grok-4");
    expect(s.provider).toBe("xai");
    // The API-facing slug keeps its slash.
    expect((s.params as { model: string }).model).toBe("xai/grok-4");
  });

  it("solves with a fake reply (no API call)", async () => {
    const fake =
      '```json\n{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}\n```';
    const solver = makeGatewayDirectSolver({ fakeReplyForTests: fake });
    const out = await solver.solve(dummyTask, dummyTask.prompt);
    expect(JSON.parse(out.vcadJson).version).toBe("0.1");
    expect(out.toolCalls).toHaveLength(1);
    expect(out.toolCalls[0].tool).toBe("gateway.chat.completions.create");
  });
});

describe("getSolver gateway routing", () => {
  it("resolves the bare id to the default model", () => {
    expect(getSolver("gateway-direct").id).toBe(
      "gateway-direct-anthropic-claude-sonnet-4.5",
    );
  });

  it("accepts the native provider/model slug", () => {
    const s = getSolver("gateway-direct-openai/gpt-5");
    expect(s.id).toBe("gateway-direct-openai-gpt-5");
    expect((s.params as { model: string }).model).toBe("openai/gpt-5");
  });

  it("accepts the filesystem-safe dashed spelling", () => {
    const s = getSolver("gateway-direct-google-gemini-2.5-pro");
    expect(s.id).toBe("gateway-direct-google-gemini-2.5-pro");
    expect((s.params as { model: string }).model).toBe("google/gemini-2.5-pro");
  });
});
