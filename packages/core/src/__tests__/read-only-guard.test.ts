import { describe, expect, it, vi } from "vitest";
import { wrapEngineWithReadOnlyGuard } from "../stores/document-store.js";
import { useUiStore } from "../stores/ui-store.js";

function fakeEngine() {
  return {
    apply: vi.fn(() => JSON.stringify({ ok: true, result: {} })),
    apply_tool: vi.fn(() => JSON.stringify({ ok: true, result: {} })),
    set_material: vi.fn(),
    get_document_json: () => JSON.stringify({ nodes: {}, roots: [] }),
    get_parts_json: () => "[]",
  };
}

describe("read-only share guard", () => {
  it("blocks DocCommand entry points and returns a no-op envelope", () => {
    const raw = fakeEngine();
    const engine = wrapEngineWithReadOnlyGuard(raw as never) as unknown as typeof raw;
    useUiStore.setState({ readOnlyShare: { token: "t" } as never });
    try {
      const env = JSON.parse(engine.apply('{"type":"undo"}'));
      expect(env).toMatchObject({ ok: true, result: { parts: [], consumedPartIds: [] } });
      engine.apply_tool("create", "{}");
      engine.set_material("1:0", "steel");
      expect(raw.apply).not.toHaveBeenCalled();
      expect(raw.apply_tool).not.toHaveBeenCalled();
      expect(raw.set_material).not.toHaveBeenCalled();
    } finally {
      useUiStore.setState({ readOnlyShare: null as never });
    }
    engine.apply('{"type":"undo"}');
    expect(raw.apply).toHaveBeenCalledTimes(1);
  });
});
