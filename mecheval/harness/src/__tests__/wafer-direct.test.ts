import { describe, expect, it } from "vitest";
import { getSolver } from "../solver.js";
import {
  DEFAULT_WAFER_DIRECT,
  makeWaferDirectSolver,
} from "../solvers/wafer-direct.js";
import type { Task } from "../task.js";

const dummyTask: Task = {
  id: "a1-cube-01",
  suite: "A",
  tier: "A1",
  title: "Centered cube",
  prompt: "Make a 25mm cube centered on the origin.",
  checks: [],
};

describe("makeWaferDirectSolver", () => {
  it("defaults to GLM-5.2", () => {
    expect(DEFAULT_WAFER_DIRECT.model).toBe("GLM-5.2");
    const s = makeWaferDirectSolver();
    expect(s.id).toBe("wafer-direct-GLM-5.2");
    expect(s.provider).toBe("wafer");
  });

  it("carries the model override into the solver id and params", () => {
    const s = makeWaferDirectSolver({ model: "GLM-5.1" });
    expect(s.id).toBe("wafer-direct-GLM-5.1");
    expect((s.params as { model: string }).model).toBe("GLM-5.1");
  });

  it("solves with a fake reply (no API call)", async () => {
    const fake =
      '```json\n{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}\n```';
    const solver = makeWaferDirectSolver({ fakeReplyForTests: fake });
    const out = await solver.solve(dummyTask, dummyTask.prompt);
    expect(JSON.parse(out.vcadJson).version).toBe("0.1");
    expect(out.toolCalls).toHaveLength(1);
    expect(out.toolCalls[0].tool).toBe("wafer.chat.completions.create");
  });
});

describe("getSolver wafer routing", () => {
  it("resolves the bare id to GLM-5.2", () => {
    expect(getSolver("wafer-direct").id).toBe("wafer-direct-GLM-5.2");
  });

  it("resolves a suffixed id to the overridden model", () => {
    expect(getSolver("wafer-direct-GLM-5.1").id).toBe("wafer-direct-GLM-5.1");
  });
});
