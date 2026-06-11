/**
 * Regression tests for per-block extraction from MCP results.
 *
 * The MCP server appends preview-handle text blocks to geometry tool
 * results for MCP Apps hosts. Joining blocks before JSON.parse corrupted
 * the extracted .vcad with trailing characters and errored every run of
 * the first post-viewer matrix — these pin the per-block behavior.
 */

import { describe, it, expect } from "vitest";
import {
  extractDocumentId,
  extractVcadJson,
} from "../solvers/claude-mcp.js";

const DOC = {
  version: "0.1",
  nodes: { "1": { id: 1, name: "x", op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } } },
  materials: {},
  part_materials: {},
  roots: [{ root: 1, material: "default" }],
};

describe("extractVcadJson", () => {
  it("parses a single-block document result", () => {
    const out = extractVcadJson({
      content: [{ type: "text", text: JSON.stringify(DOC) }],
    });
    expect(JSON.parse(out).roots).toHaveLength(1);
  });

  it("survives an appended preview-handle block", () => {
    const out = extractVcadJson({
      content: [
        { type: "text", text: JSON.stringify(DOC) },
        { type: "text", text: JSON.stringify({ document_id: "doc_123" }) },
      ],
    });
    const parsed = JSON.parse(out);
    expect(parsed.nodes["1"].op.type).toBe("Cube");
    expect("document_id" in parsed).toBe(false);
  });

  it("skips non-document blocks regardless of order", () => {
    const out = extractVcadJson({
      content: [
        { type: "text", text: JSON.stringify({ document_id: "doc_123" }) },
        { type: "text", text: JSON.stringify(DOC) },
      ],
    });
    expect(JSON.parse(out).version).toBe("0.1");
  });

  it("throws when no block parses as a Document", () => {
    expect(() =>
      extractVcadJson({
        content: [{ type: "text", text: JSON.stringify({ document_id: "doc_123" }) }],
      }),
    ).toThrow(/no parseable Document/);
  });
});

describe("extractDocumentId", () => {
  it("finds the id in the first block", () => {
    expect(
      extractDocumentId({
        content: [
          { type: "text", text: JSON.stringify({ document_id: "doc_42", parts: 0 }) },
        ],
      }),
    ).toBe("doc_42");
  });

  it("finds the id even when a non-JSON block precedes it", () => {
    expect(
      extractDocumentId({
        content: [
          { type: "text", text: "CAD document ready." },
          { type: "text", text: JSON.stringify({ document_id: "doc_7" }) },
        ],
      }),
    ).toBe("doc_7");
  });

  it("throws with context when absent", () => {
    expect(() =>
      extractDocumentId({ content: [{ type: "text", text: "nope" }] }),
    ).toThrow(/did not return a JSON document_id/);
  });
});
