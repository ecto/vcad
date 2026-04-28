import { describe, expect, it } from "vitest";
import {
  buildUserPrompt,
  extractVcadJson,
  makeClaudeDirectSolver,
} from "../solvers/claude-direct.js";
import type { Task } from "../task.js";

const dummyTask: Task = {
  id: "a1-cube-01",
  suite: "A",
  tier: "A1",
  title: "Centered cube",
  prompt: "Make a 25mm cube centered on the origin.",
  checks: [],
};

describe("extractVcadJson", () => {
  it("pulls a fenced ```json block", () => {
    const reply = "Here you go:\n\n```json\n{\"hi\":1}\n```\n\nDone.";
    expect(extractVcadJson(reply)).toBe('{"hi":1}');
  });

  it("pulls an un-tagged fenced block", () => {
    const reply = "```\n{\"hi\":1}\n```";
    expect(extractVcadJson(reply)).toBe('{"hi":1}');
  });

  it("falls back to balanced braces when no fence is present", () => {
    const reply = "Sure: {\"a\":{\"b\":1},\"c\":2} that's it.";
    const out = extractVcadJson(reply);
    expect(JSON.parse(out)).toEqual({ a: { b: 1 }, c: 2 });
  });

  it("throws on a reply with no JSON", () => {
    expect(() => extractVcadJson("hi there")).toThrow();
  });
});

describe("buildUserPrompt", () => {
  it("includes task id, tier, and the task prompt", () => {
    const p = buildUserPrompt(dummyTask, dummyTask.prompt);
    expect(p).toContain("Task ID: a1-cube-01");
    expect(p).toContain("Tier: A1");
    expect(p).toContain("Make a 25mm cube");
  });
});

describe("makeClaudeDirectSolver", () => {
  it("solves with a fake reply (no API call)", async () => {
    const fake = '```json\n{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}\n```';
    const solver = makeClaudeDirectSolver({
      model: "claude-opus-4-7",
      maxOutputTokens: 100,
      fakeReplyForTests: fake,
    });
    const out = await solver.solve(dummyTask, dummyTask.prompt);
    const parsed = JSON.parse(out.vcadJson);
    expect(parsed.version).toBe("0.1");
    expect(out.toolCalls).toHaveLength(1);
    expect(out.toolCalls[0].tool).toBe("anthropic.messages.create");
  });

  it("carries the model override into the solver id and params", () => {
    const s = makeClaudeDirectSolver({ model: "claude-haiku-4-5-20251001" });
    expect(s.id).toBe("claude-direct-claude-haiku-4-5-20251001");
    expect(s.provider).toBe("anthropic");
    expect((s.params as { model: string }).model).toBe(
      "claude-haiku-4-5-20251001",
    );
  });
});
