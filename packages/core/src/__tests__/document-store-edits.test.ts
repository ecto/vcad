/**
 * PCB / schematic / design-constraint edits in the document store go through
 * the engine's Rust lowering (`apply_tool`) — the store holds no edit logic of
 * its own. The engine is faked here; the lowerings themselves are unit-tested
 * in crates/vcad-app/src/doc_edits.rs.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useDocumentStore } from "../stores/document-store.js";

const doc = (tag: string) =>
  ({ version: "0.1", nodes: {}, materials: {}, part_materials: {}, roots: [], tag }) as never;

let calls: Array<{ tool: string; args: unknown }> = [];
let reply: unknown;

beforeEach(() => {
  calls = [];
  reply = { ok: true, result: { document: doc("after"), parts: [], consumedPartIds: [] } };
  useDocumentStore.setState({
    document: doc("before"),
    _crdtEngine: {
      apply_tool: (tool: string, args: string) => {
        calls.push({ tool, args: JSON.parse(args) });
        return JSON.stringify(reply);
      },
      apply: vi.fn(),
    } as never,
  });
});

const tag = () => (useDocumentStore.getState().document as unknown as { tag: string }).tag;

describe("document-store blob edits run on the engine", () => {
  it("maps each store method to its engine tool", () => {
    const s = useDocumentStore.getState();
    s.moveFootprint(0, 2, { x: 5, y: 6, z: 0 });
    s.rotateFootprint(0, 2, 90);
    s.flipFootprint(0, 2);
    s.removeTrace(0, 1);
    s.removeVia(0, 3);
    s.removeFootprint(0, 4);
    s.addSchematicWire({ start: { x: 0, y: 0 }, end: { x: 1, y: 0 } } as never);
    s.removeSchematicWire(0);
    s.removeSchematicLabel(1);
    s.addSchematicJunction({ position: { x: 2, y: 2 } } as never);
    s.moveSchematicComponentWithWires(0, { x: 1, y: 2, z: 0 }, [
      { wireIdx: 3, endpoint: "end", pos: { x: 1, y: 2 } },
    ]);
    s.removeDesignConstraint("c2");
    expect(calls).toEqual([
      { tool: "pcb_move_footprint", args: { index: 2, position: { x: 5, y: 6 } } },
      { tool: "pcb_rotate_footprint", args: { index: 2, angle: 90 } },
      { tool: "pcb_flip_footprint", args: { index: 2 } },
      { tool: "pcb_remove_trace", args: { index: 1 } },
      { tool: "pcb_remove_via", args: { index: 3 } },
      { tool: "pcb_remove_footprint", args: { index: 4 } },
      { tool: "sch_add_wire", args: { wire: { start: { x: 0, y: 0 }, end: { x: 1, y: 0 } } } },
      { tool: "sch_remove_wire", args: { index: 0 } },
      { tool: "sch_remove_label", args: { index: 1 } },
      { tool: "sch_add_junction", args: { junction: { position: { x: 2, y: 2 } } } },
      {
        tool: "sch_move_component",
        args: {
          index: 0,
          position: { x: 1, y: 2 },
          wire_updates: [{ wire_index: 3, endpoint: "end", position: { x: 1, y: 2 } }],
        },
      },
      { tool: "remove_design_constraint", args: { id: "c2" } },
    ]);
    expect(tag()).toBe("after");
  });

  it("picks trace/via fields and passes constraint patches through", () => {
    const s = useDocumentStore.getState();
    s.addTrace(0, {
      start: { x: 0, y: 1, z: 9 }, end: { x: 2, y: 3, z: 9 }, width: 0.2, layer: "F.Cu", net: "GND",
    });
    s.addVia(0, {
      position: { x: 1, y: 1, z: 9 }, diameter: 0.6, drill: 0.3,
      startLayer: "F.Cu", endLayer: "B.Cu", net: "GND",
    });
    s.updateDesignConstraint("c1", { value: "w / 2" });
    expect(calls[0]).toEqual({
      tool: "pcb_add_trace",
      args: { trace: { start: { x: 0, y: 1 }, end: { x: 2, y: 3 }, width: 0.2, layer: "F.Cu", net: "GND" } },
    });
    expect(calls[1]).toEqual({
      tool: "pcb_add_via",
      args: {
        via: { position: { x: 1, y: 1 }, diameter: 0.6, drill: 0.3, startLayer: "F.Cu", endLayer: "B.Cu", net: "GND" },
      },
    });
    expect(calls[2]).toEqual({ tool: "update_design_constraint", args: { id: "c1", value: "w / 2" } });
  });

  it("routes board and schematic-component edits to the engine", () => {
    const s = useDocumentStore.getState();
    s.resizeBoard(40, 30);
    s.setBoardOutline({ vertices: [] } as never);
    s.addSchematicComponent({ ref: "R1" } as never);
    s.updateSchematicComponent(0, { value: "22k" } as never);
    s.removeSchematicComponent(0);
    expect(calls).toEqual([
      { tool: "pcb_resize", args: { width: 40, height: 30 } },
      { tool: "pcb_set_outline", args: { outline: { vertices: [] } } },
      { tool: "sch_add_component", args: { component: { ref: "R1" } } },
      { tool: "sch_update_component", args: { index: 0, updates: { value: "22k" } } },
      { tool: "sch_remove_component", args: { index: 0 } },
    ]);
  });

  it("leaves the document untouched when the engine refuses an edit", () => {
    reply = { ok: false, error: { kind: "invalid", message: "index 9 out of range" } };
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    useDocumentStore.getState().removeTrace(0, 9);
    expect(useDocumentStore.getState().addDesignConstraint({ kind: {} } as never)).toBeNull();
    expect(tag()).toBe("before");
    expect(warn).toHaveBeenCalledWith("[document-store] pcb_remove_trace: index 9 out of range");
    warn.mockRestore();
  });
});
