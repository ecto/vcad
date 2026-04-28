import { describe, expect, it } from "vitest";
import { defaultCubeSolver, getSolver } from "../solver.js";

const dummyTask = {
  id: "x",
  suite: "A" as const,
  tier: "A1",
  title: "x",
  prompt: "ignored",
  checks: [],
};

describe("defaultCubeSolver", () => {
  it("emits a parseable .vcad with a 50mm cube", async () => {
    const out = await defaultCubeSolver.solve(dummyTask, "anything");
    const parsed = JSON.parse(out.vcadJson);
    expect(parsed.version).toBe("0.1");
    const cube = parsed.nodes["1"];
    expect(cube.op.type).toBe("Cube");
    expect(cube.op.size).toEqual({ x: 50.0, y: 50.0, z: 50.0 });
  });

  it("getSolver finds it under either casing", () => {
    expect(getSolver("default-cube").id).toBe("default-cube");
    expect(getSolver("DEFAULT_CUBE").id).toBe("default-cube");
  });

  it("getSolver throws on unknown ids", () => {
    expect(() => getSolver("not-a-real-solver")).toThrow();
  });
});
