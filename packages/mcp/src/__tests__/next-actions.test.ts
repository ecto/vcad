import { describe, it, expect } from "vitest";
import {
  suggestNextActions,
  buildErrorResult,
  enrichErrorResult,
} from "../tools/next-actions.js";

describe("suggestNextActions", () => {
  it("steers a kernel trap toward different inputs (no blind retry tool)", () => {
    const actions = suggestNextActions("fillet", { document_id: "d" }, "unreachable", {
      kernelTrap: true,
    });
    expect(actions).toHaveLength(1);
    expect(actions[0].action.toLowerCase()).toContain("kernel reset");
    expect(actions[0].tool).toBeUndefined();
  });

  it("points an unknown document_id at open_document", () => {
    const actions = suggestNextActions(
      "update",
      { document_id: "doc_x" },
      "Unknown document_id: doc_x",
    );
    expect(actions[0].tool).toBe("open_document");
  });

  it("points a missing/bad part_id at read, carrying the document_id", () => {
    const a1 = suggestNextActions("delete", { document_id: "d1" }, "delete: missing `part_id`");
    expect(a1[0].tool).toBe("read");
    expect(a1[0].args).toEqual({ document_id: "d1" });

    const a2 = suggestNextActions("read", { document_id: "d1" }, 'read: no part with id "7"');
    expect(a2[0].tool).toBe("read");
  });

  it("returns the type's param hint for a malformed create", () => {
    const actions = suggestNextActions(
      "create",
      { document_id: "d", type: "cylinder", params: {} },
      "missing field `radius`",
    );
    // First action carries the cylinder shape; a follow-up names the field.
    expect(actions[0].tool).toBe("create");
    expect(actions[0].action).toContain("radius");
    expect(actions.some((a) => a.action.includes('"radius"'))).toBe(true);
  });

  it("adds a children-first hint for a malformed boolean", () => {
    const actions = suggestNextActions(
      "create",
      { document_id: "d", type: "difference", params: {} },
      "invalid node reference",
    );
    expect(actions.some((a) => a.action.toLowerCase().includes("child nodes first"))).toBe(true);
  });

  it("falls back to the catalog hint for an unknown create type", () => {
    const actions = suggestNextActions("create", { document_id: "d", type: "blob" }, "bad type");
    expect(actions[0].tool).toBe("create");
    expect(actions[0].action.toLowerCase()).toContain("type catalog");
  });

  it("marks a planner-unavailable error as non-recoverable from the client", () => {
    const actions = suggestNextActions("create", {}, "Rust planner unavailable");
    expect(actions[0].tool).toBeUndefined();
    expect(actions[0].action.toLowerCase()).toContain("misconfiguration");
  });

  it("routes a missing schematic to create_schematic", () => {
    const actions = suggestNextActions(
      "place_components",
      { document_id: "d" },
      "Error: Document has no schematic",
    );
    expect(actions[0].tool).toBe("create_schematic");
    expect(actions[0].args).toEqual({ document_id: "d" });
  });

  it("routes a missing board to place_components", () => {
    const actions = suggestNextActions(
      "route_nets",
      { document_id: "d" },
      "Error: Document has no PCB — run place_components first",
    );
    expect(actions[0].tool).toBe("place_components");
  });

  it("routes an unknown catalog part to search_parts (not read)", () => {
    const actions = suggestNextActions(
      "place_part",
      { document_id: "d" },
      'place_part: unknown part "m3_screw"',
    );
    expect(actions[0].tool).toBe("search_parts");
  });

  it("lets a generic update error fall through to the inspect floor", () => {
    // `update` has no `type`, so it must not get a create-flavored catalog hint.
    const actions = suggestNextActions("update", { document_id: "d" }, "some update failure");
    expect(actions[0].tool).toBe("read");
    expect(actions[0].args).toEqual({ document_id: "d" });
  });

  it("falls back to inspect-then-retry for an unrecognized error", () => {
    const actions = suggestNextActions("export_cad", { document_id: "d" }, "weird failure");
    expect(actions[0].tool).toBe("read");
    expect(actions[0].args).toEqual({ document_id: "d" });
  });

  it("omits args when no document_id is in context", () => {
    const actions = suggestNextActions("export_cad", {}, "weird failure");
    expect(actions[0].tool).toBe("read");
    expect(actions[0].args).toBeUndefined();
  });
});

describe("buildErrorResult", () => {
  it("carries the message, a machine-readable tail, and structured actions", () => {
    const res = buildErrorResult("delete", { document_id: "d1" }, "delete: missing `part_id`");
    expect(res.isError).toBe(true);
    const text = res.content[0].text;
    expect(text).toContain("Error: delete: missing `part_id`");
    expect(text).toContain("next_actions:");
    // The tail is valid JSON the agent can parse.
    const tail = text.slice(text.indexOf("next_actions:") + "next_actions:".length).trim();
    expect(JSON.parse(tail)).toEqual(res.structuredContent.next_actions);
    expect(res.structuredContent.next_actions[0].tool).toBe("read");
    expect(res.structuredContent.error).toBe("delete: missing `part_id`");
  });

  it("uses the kernel-trap headline for a trap", () => {
    const res = buildErrorResult("fillet", { document_id: "d" }, "unreachable", {
      kernelTrap: true,
    });
    expect(res.content[0].text).toContain("kernel trap during 'fillet'");
    expect(res.content[0].text).toContain("was reset");
  });
});

describe("enrichErrorResult (for tools that return isError instead of throwing)", () => {
  it("attaches next_actions to a plain-text ECAD error (appended tail)", () => {
    const result = {
      content: [{ type: "text", text: "Error: Document has no schematic" }],
      isError: true,
    };
    enrichErrorResult(result, "place_components", { document_id: "d" });
    expect(result.content[0].text).toContain("next_actions:");
    expect(
      (result as { structuredContent?: { next_actions?: Array<{ tool?: string }> } })
        .structuredContent?.next_actions?.[0].tool,
    ).toBe("create_schematic");
  });

  it("injects next_actions INTO a JSON error body, keeping it parseable", () => {
    const result = {
      content: [
        { type: "text", text: JSON.stringify({ error: 'place_part: unknown part "x"' }) },
      ],
      isError: true,
    };
    enrichErrorResult(result, "place_part", { document_id: "d" });
    const parsed = JSON.parse(result.content[0].text) as {
      error: string;
      next_actions: Array<{ tool?: string }>;
    };
    expect(parsed.error).toContain("unknown part");
    expect(parsed.next_actions[0].tool).toBe("search_parts");
  });

  it("is idempotent — a result already carrying next_actions is untouched", () => {
    const result = {
      content: [{ type: "text", text: "Error: x\nnext_actions: [{}]" }],
      structuredContent: { next_actions: [{ action: "already here" }] },
      isError: true as const,
    };
    const before = result.content[0].text;
    enrichErrorResult(result, "route_nets", {});
    expect(result.content[0].text).toBe(before);
  });

  it("skips carve-outs (unknown tool / disabled pack)", () => {
    const result = { content: [{ type: "text", text: "Unknown tool: frobnicate" }], isError: true };
    enrichErrorResult(result, "frobnicate", {});
    expect(result.content[0].text).toBe("Unknown tool: frobnicate");
    expect((result as { structuredContent?: unknown }).structuredContent).toBeUndefined();
  });

  it("is a no-op on a successful result", () => {
    const result = { content: [{ type: "text", text: "ok" }], isError: false };
    enrichErrorResult(result, "create", {});
    expect(result.content[0].text).toBe("ok");
  });
});
