import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { Document } from "@vcad/ir";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";
import {
  listParameters,
  parameterGradient,
  setParameters,
} from "../tools/parameters.js";

/**
 * Coverage for the parametric-parameter MCP surface (the differentiable seam):
 *  - list_parameters reads named parameters from a fixture doc
 *  - set_parameters batch-updates them and reports a `changed` diff
 *  - parameter_gradient's analytic d(volume)/dθ matches a central finite
 *    difference of the tool's own reported volume, driven end-to-end through
 *    the real MCP server.
 */

const HEIGHT = 8;
const SEGS = 64;

/** A parametric cylinder whose radius is bound to the named parameter `r`. */
function cylinderDoc(r: number): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "disc",
        op: { type: "Cylinder", radius: 1, height: HEIGHT, segments: SEGS },
      },
    },
    materials: {
      default: {
        name: "default",
        color: [0.8, 0.8, 0.8],
        metallic: 0,
        roughness: 0.5,
      },
    },
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
    parameters: { r: { value: r } },
    bindings: { "1:radius": "r" },
  } as unknown as Document;
}

function json(result: { content: Array<{ type: string; text: string }> }) {
  return JSON.parse(result.content[0].text) as Record<string, unknown>;
}

describe("parameter tools (handlers)", () => {
  let engine: Engine;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    documents.clear();
  });

  it("list_parameters returns named parameters (inline doc)", () => {
    const out = json(listParameters({ document: cylinderDoc(10) }));
    expect(out.count).toBe(1);
    const params = out.parameters as Array<Record<string, unknown>>;
    expect(params[0].name).toBe("r");
    expect(params[0].value).toBe(10);
    expect(params[0].resolved).toBeCloseTo(10, 9);
  });

  it("set_parameters batch-updates with a changed diff", () => {
    documents.set("doc_test", cylinderDoc(10));
    const out = json(
      setParameters({ document_id: "doc_test", parameters: { r: 15 } }, engine),
    );
    expect(out.updated).toBe(1);
    const changed = out.changed as Array<Record<string, unknown>>;
    expect(changed[0]).toMatchObject({ name: "r", previous: 10, value: 15 });
    // The mutation is applied in place on the live session document.
    const doc = documents.get("doc_test") as Document;
    expect(doc.parameters?.r.value).toBe(15);
  });

  it("set_parameters rejects unknown parameters without partial application", () => {
    documents.set("doc_test", cylinderDoc(10));
    expect(() =>
      setParameters(
        { document_id: "doc_test", parameters: { r: 12, nope: 3 } },
        engine,
      ),
    ).toThrow(/Unknown parameter/);
    // r must be untouched — the batch validated before applying anything.
    const doc = documents.get("doc_test") as Document;
    expect(doc.parameters?.r.value).toBe(10);
  });

  it("parameter_gradient d(volume)/dr matches a central finite difference", () => {
    const grad = (r: number) =>
      json(
        parameterGradient(
          { document: cylinderDoc(r), parameter: "r" },
          engine,
        ),
      ).parts as Array<Record<string, number>>;

    const h = 1e-3;
    const g0 = grad(10);
    expect(g0.length).toBe(1);

    const plus = grad(10 + h);
    const minus = grad(10 - h);
    const fd = (plus[0].volume - minus[0].volume) / (2 * h);
    const rel = Math.abs(g0[0].dVolume - fd) / Math.abs(fd);
    expect(rel).toBeLessThan(1e-3);

    // Mass tracks density (default 1), and a centered disc's centroid is
    // r-invariant.
    expect(g0[0].dCentroid[0]).toBeCloseTo(0, 4);
    expect(g0[0].dCentroid[1]).toBeCloseTo(0, 4);
    expect(g0[0].dCentroid[2]).toBeCloseTo(0, 4);
    // A 64-gon disc spans [-r, r] on x/y → extent 2r, growing at ~2·dr.
    expect(g0[0].dBboxExtents[0]).toBeCloseTo(2, 1);
  });

  it("parameter_gradient errors on an unknown parameter", () => {
    expect(() =>
      parameterGradient({ document: cylinderDoc(10), parameter: "nope" }, engine),
    ).toThrow();
  });
});

describe("parameter tools (end-to-end through MCP server)", () => {
  let engine: Engine;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    documents.clear();
  });

  async function connect() {
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "test", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
    return client;
  }

  function textOf(result: unknown) {
    const content = (
      result as { content: Array<{ type: string; text: string }> }
    ).content;
    return JSON.parse(content[0].text) as Record<string, unknown>;
  }

  it("open → set_parameters → parameter_gradient over the wire", async () => {
    const client = await connect();

    const opened = textOf(
      await client.callTool({
        name: "open_document",
        arguments: { initial: cylinderDoc(10) },
      }),
    );
    const documentId = String(opened.document_id);

    const listed = textOf(
      await client.callTool({
        name: "list_parameters",
        arguments: { document_id: documentId },
      }),
    );
    expect(listed.count).toBe(1);

    const set = textOf(
      await client.callTool({
        name: "set_parameters",
        arguments: { document_id: documentId, parameters: { r: 12 } },
      }),
    );
    expect((set.changed as Array<Record<string, unknown>>)[0]).toMatchObject({
      name: "r",
      previous: 10,
      value: 12,
    });

    // Gradient at the updated value, over the wire.
    const grad = textOf(
      await client.callTool({
        name: "parameter_gradient",
        arguments: { document_id: documentId, parameter: "r" },
      }),
    );
    const parts = grad.parts as Array<Record<string, number>>;
    expect(parts.length).toBe(1);
    // Closed form dV/dr for the 64-gon prism is 2·k·r·h > 0 at r = 12.
    expect(parts[0].dVolume).toBeGreaterThan(0);
    expect(parts[0].volume).toBeGreaterThan(0);
  });
});
