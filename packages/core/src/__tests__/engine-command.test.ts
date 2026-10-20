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

function legacyEngine(): CommandEngine {
  return {
    add_feature: vi.fn(() => state({ createdFeatureId: "1:0" })),
    delete_feature_by_id: vi.fn(() => state()),
    set_translation: vi.fn(() => state()),
    set_rotation: vi.fn(() => state()),
    set_scale: vi.fn(() => state()),
    set_material: vi.fn(() => state()),
    set_visible: vi.fn(() => state()),
    rename_feature: vi.fn(() => state()),
    set_joint_state: vi.fn(() => state()),
    import_ir: vi.fn(() => state()),
    undo: vi.fn(() => state()),
    redo: vi.fn(() => state()),
  };
}

describe("applyCommand", () => {
  it("sends the command JSON to engine.apply and unwraps the result", () => {
    const engine = legacyEngine();
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
    expect(engine.add_feature).not.toHaveBeenCalled();
  });

  it("throws DocCommandError when the engine refuses", () => {
    const engine = legacyEngine();
    engine.apply = () =>
      JSON.stringify({
        ok: false,
        error: { kind: "unknown_id", id: "9:9", message: "no feature with id `9:9`" },
      });
    const run = () => applyCommand(engine, { type: "delete_feature", id: "9:9" });
    expect(run).toThrow(DocCommandError);
    expect(run).toThrow("delete_feature: no feature with id `9:9`");
  });

  it("falls back to per-method calls on a pre-apply kernel", () => {
    const engine = legacyEngine();
    applyCommand(engine, { type: "set_translation", id: "1:0", offset: [1, 2, 3] });
    expect(engine.set_translation).toHaveBeenCalledWith("1:0", 1, 2, 3);
    const named = applyCommand(engine, {
      type: "add_feature",
      name: "Plate",
      feature: { type: "Cube", size_x: 1, size_y: 1, size_z: 1 },
    });
    expect(engine.rename_feature).toHaveBeenCalledWith("1:0", "Plate");
    expect(named.createdFeatureId).toBe("1:0");
  });
});
