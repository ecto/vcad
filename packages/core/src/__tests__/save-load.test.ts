import { describe, it, expect } from "vitest";
import { createDocument } from "@vcad/ir";
import {
  buildVcadFileFromState,
  deriveParts,
  getDocumentForDisplay,
  parseVcadFile,
  serializeDocument,
  type VcadFile,
} from "../utils/save-load.js";

function makeDoc(nodes: Record<string, unknown>, rootId: number) {
  const doc = createDocument();
  doc.nodes = nodes as typeof doc.nodes;
  doc.roots = [{ root: rootId, material: "default" }];
  return doc;
}

describe("deriveParts transform chain", () => {
  it("handles root Rotate -> Cube", () => {
    const doc = makeDoc(
      {
        "1": {
          id: 1,
          name: "Root",
          op: { type: "Rotate", child: 2, angles: { x: 0, y: 0, z: 0 } },
        },
        "2": {
          id: 2,
          name: "Cube",
          op: { type: "Cube", size: { x: 1, y: 2, z: 3 } },
        },
      },
      1,
    );

    const parts = deriveParts(doc);
    expect(parts).toHaveLength(1);
    expect(parts[0]!.kind).toBe("cube");
    expect(parts[0]!.primitiveNodeId).toBe(2);
    expect(parts[0]!.translateNodeId).toBe(1);
    expect(parts[0]!.rotateNodeId).toBe(1);
  });

  it("handles root Scale -> Cube", () => {
    const doc = makeDoc(
      {
        "1": {
          id: 1,
          name: "Root",
          op: { type: "Scale", child: 2, factor: { x: 1, y: 1, z: 1 } },
        },
        "2": {
          id: 2,
          name: "Cube",
          op: { type: "Cube", size: { x: 2, y: 2, z: 2 } },
        },
      },
      1,
    );

    const parts = deriveParts(doc);
    expect(parts).toHaveLength(1);
    expect(parts[0]!.kind).toBe("cube");
    expect(parts[0]!.primitiveNodeId).toBe(2);
    expect(parts[0]!.scaleNodeId).toBe(1);
    expect(parts[0]!.translateNodeId).toBe(1);
  });

  it("handles Translate -> Rotate -> Scale -> Cube", () => {
    const doc = makeDoc(
      {
        "1": {
          id: 1,
          name: "Root",
          op: {
            type: "Translate",
            child: 2,
            offset: { x: 1, y: 2, z: 3 },
          },
        },
        "2": {
          id: 2,
          name: "Rotate",
          op: { type: "Rotate", child: 3, angles: { x: 0, y: 0, z: 0 } },
        },
        "3": {
          id: 3,
          name: "Scale",
          op: { type: "Scale", child: 4, factor: { x: 1, y: 1, z: 1 } },
        },
        "4": {
          id: 4,
          name: "Cube",
          op: { type: "Cube", size: { x: 4, y: 5, z: 6 } },
        },
      },
      1,
    );

    const parts = deriveParts(doc);
    expect(parts).toHaveLength(1);
    expect(parts[0]!.kind).toBe("cube");
    expect(parts[0]!.primitiveNodeId).toBe(4);
    expect(parts[0]!.translateNodeId).toBe(1);
    expect(parts[0]!.rotateNodeId).toBe(2);
    expect(parts[0]!.scaleNodeId).toBe(3);
  });
});

// Fake CRDT-save output — shape matches what `CrdtDocument::save()` emits
// (JSON bytes with replica_id/ops/features…). We don't exercise the real
// CRDT here; this is a TS-layer detector/roundtrip test.
const FAKE_CRDT_JSON = JSON.stringify({
  replica_id: 1,
  hlc: { timestamp: 0, counter: 0, replica: 1 },
  next_seq: 0,
  ops: [],
  features: [],
  undo_stacks: [],
  redo_stacks: [],
  clock: [],
});

describe("parseVcadFile format discrimination", () => {
  it("detects v0.4 CRDT by replica_id + ops presence", () => {
    const file = parseVcadFile(FAKE_CRDT_JSON);
    expect(file.kind).toBe("crdt");
    if (file.kind !== "crdt") throw new Error("unreachable");
    expect(file.version).toBe("0.4");
    // Bytes roundtrip: the parser keeps the original JSON verbatim so the
    // engine can load them without re-encoding ambiguity.
    expect(new TextDecoder().decode(file.crdtBytes)).toBe(FAKE_CRDT_JSON);
  });

  it("detects v0.1 legacy JSON shape", () => {
    const doc = createDocument();
    const legacyJson = JSON.stringify({
      version: "0.1",
      document: doc,
      parts: [],
      consumedParts: {},
      nextNodeId: 1,
      nextPartNum: 1,
    });
    const file = parseVcadFile(legacyJson);
    // Either the WASM module or the TS fallback handles it; both yield the
    // `legacy` variant (the wrapping is the same).
    expect(file.kind).toBe("legacy");
  });

  it("throws on malformed v0.4 JSON", () => {
    const broken = '{"replica_id":1,"ops":[';
    expect(() => parseVcadFile(broken)).toThrow();
  });
});

describe("serializeDocument", () => {
  it("prefers loonSource over CRDT bytes", () => {
    const loon = "(box 10 10 10)";
    expect(
      serializeDocument({
        loonSource: loon,
        crdtBytes: new TextEncoder().encode("ignored"),
      }),
    ).toBe(loon);
  });

  it("decodes CRDT bytes as UTF-8 when loon absent", () => {
    expect(
      serializeDocument({
        crdtBytes: new TextEncoder().encode(FAKE_CRDT_JSON),
      }),
    ).toBe(FAKE_CRDT_JSON);
  });

  it("reads from `_crdtEngine.save()` when no bytes provided", () => {
    const engine = {
      save: () => new TextEncoder().encode(FAKE_CRDT_JSON),
    };
    expect(serializeDocument({ _crdtEngine: engine })).toBe(FAKE_CRDT_JSON);
  });

  it("throws when neither source is available", () => {
    expect(() => serializeDocument({})).toThrow(/no CRDT bytes or loon source/);
  });
});

describe("buildVcadFileFromState", () => {
  it("builds a loon variant when loonSource is present", () => {
    const file = buildVcadFileFromState({ loonSource: "(box 1 2 3)" });
    expect(file?.kind).toBe("loon");
  });

  it("builds a crdt variant when only the engine is present", () => {
    const engine = { save: () => new TextEncoder().encode(FAKE_CRDT_JSON) };
    const file = buildVcadFileFromState({ _crdtEngine: engine });
    expect(file?.kind).toBe("crdt");
    if (file?.kind === "crdt") {
      expect(new TextDecoder().decode(file.crdtBytes)).toBe(FAKE_CRDT_JSON);
    }
  });

  it("returns null when neither is available", () => {
    expect(buildVcadFileFromState({})).toBeNull();
  });
});

describe("getDocumentForDisplay", () => {
  it("returns null for CRDT files (no materialized document)", () => {
    const file: VcadFile = {
      kind: "crdt",
      version: "0.4",
      crdtBytes: new TextEncoder().encode(FAKE_CRDT_JSON),
    };
    expect(getDocumentForDisplay(file)).toBeNull();
  });

  it("returns the embedded document for legacy files", () => {
    const doc = createDocument();
    const file: VcadFile = {
      kind: "legacy",
      version: "0.1",
      document: doc,
      parts: [],
      nextNodeId: 1,
    };
    expect(getDocumentForDisplay(file)).toBe(doc);
  });
});

describe("serialize → parse roundtrip", () => {
  it("preserves a CRDT payload byte-for-byte", () => {
    const text = serializeDocument({
      crdtBytes: new TextEncoder().encode(FAKE_CRDT_JSON),
    });
    const parsed = parseVcadFile(text);
    expect(parsed.kind).toBe("crdt");
    if (parsed.kind === "crdt") {
      expect(new TextDecoder().decode(parsed.crdtBytes)).toBe(FAKE_CRDT_JSON);
    }
  });
});

describe("wrapLegacyWasmResult Map normalization", () => {
  it("converts serde_wasm_bindgen Maps back to plain objects", async () => {
    const { wrapLegacyWasmResult } = await import("../utils/save-load.js");
    // serde_wasm_bindgen serializes Rust HashMaps (nodes, partDefs,
    // materials) as JS Maps; JSON.stringify turns a Map into {} which used
    // to silently drop every node of a WASM-parsed document.
    const wasmShaped = {
      version: "0.2",
      document: {
        version: "0.1",
        nodes: new Map([
          ["0", { id: 0, op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } }],
        ]),
        materials: new Map(),
        part_materials: new Map(),
        roots: [],
        partDefs: new Map([
          ["p", { id: "p", name: "p", root: 0 }],
        ]),
        instances: [{ id: "i", partDefId: "p" }],
        joints: [],
      },
      parts: [],
      nextNodeId: 1,
    };
    const file = wrapLegacyWasmResult(wasmShaped);
    if (file.kind !== "legacy") throw new Error("expected legacy kind");
    expect(Object.keys(file.document.nodes)).toEqual(["0"]);
    expect(Object.keys(file.document.partDefs ?? {})).toEqual(["p"]);
    // The whole point: stringify must not lose geometry.
    const round = JSON.parse(JSON.stringify(file.document));
    expect(Object.keys(round.nodes)).toHaveLength(1);
    expect(Object.keys(round.partDefs)).toHaveLength(1);
  });
});
