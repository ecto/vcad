import { describe, expect, it, vi } from "vitest";
import {
  applyCommand,
  DocCommandError,
  type CommandEngine,
  type CommandResult,
} from "../stores/engine-command.js";

const state = (extra: Partial<CommandResult> = {}): CommandResult => ({
  document: { version: "0.1", nodes: {}, materials: {}, part_materials: {}, roots: [] } as never,
  parts: [],
  consumedPartIds: [],
  ...extra,
});

function fakeEngine(): CommandEngine {
  return {
    apply: vi.fn(() => JSON.stringify({ ok: true, result: state() })),
    apply_tool: vi.fn(() => JSON.stringify({ ok: true, result: state() })),
  };
}

describe("applyCommand", () => {
  it("sends the command JSON to engine.apply and unwraps the result", () => {
    const engine = fakeEngine();
    engine.apply = vi.fn(() =>
      JSON.stringify({ ok: true, result: state({ createdFeatureId: "7:1" }) }),
    );
    const r = applyCommand(engine, {
      type: "add_feature",
      feature: { type: "Cube", size_x: 1, size_y: 2, size_z: 3 },
    });
    expect(r.createdFeatureId).toBe("7:1");
    expect(JSON.parse((engine.apply as ReturnType<typeof vi.fn>).mock.calls[0]![0])).toEqual({
      type: "add_feature",
      feature: { type: "Cube", size_x: 1, size_y: 2, size_z: 3 },
    });
  });

  it("throws DocCommandError when the engine refuses", () => {
    const engine = fakeEngine();
    engine.apply = () =>
      JSON.stringify({
        ok: false,
        error: { kind: "unknown_id", id: "9:9", message: "no feature with id `9:9`" },
      });
    const run = () => applyCommand(engine, { type: "delete_feature", id: "9:9" });
    expect(run).toThrow(DocCommandError);
    expect(run).toThrow("delete_feature: no feature with id `9:9`");
  });
});
