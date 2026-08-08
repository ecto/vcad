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
  sensitivity,
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

  it("set_parameters batch-updates with a changed diff", async () => {
    documents.set("doc_test", cylinderDoc(10));
    const out = json(
      await setParameters({ document_id: "doc_test", parameters: { r: 15 } }, engine),
    );
    expect(out.updated).toBe(1);
    const changed = out.changed as Array<Record<string, unknown>>;
    expect(changed[0]).toMatchObject({ name: "r", previous: 10, value: 15 });
    // The mutation is applied in place on the live session document.
    const doc = documents.get("doc_test") as Document;
    expect(doc.parameters?.r.value).toBe(15);
  });

  it("set_parameters rejects unknown parameters without partial application", async () => {
    documents.set("doc_test", cylinderDoc(10));
    await expect(
      setParameters(
        { document_id: "doc_test", parameters: { r: 12, nope: 3 } },
        engine,
      ),
    ).rejects.toThrow(/Unknown parameter/);
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

/**
 * A plate with a centred through-hole: two named parameters (`hole_r`,
 * `plate_t`) so ranking has something to rank, and a real topology boundary
 * at hole_r = 10 for the trust-radius search to find.
 */
function plateDoc(holeR: number, plateT: number): Document {
  return {
    version: "0.1",
    nodes: {
      "1": { id: 1, name: "plate", op: { type: "Cube", size: { x: 20, y: 20, z: 10 } } },
      "2": {
        id: 2,
        name: "drill",
        op: { type: "Cylinder", radius: 1, height: 40, segments: 48 },
      },
      "3": {
        id: 3,
        name: "drill_at",
        op: { type: "Translate", child: 2, offset: { x: 10, y: 10, z: -10 } },
      },
      "4": { id: 4, name: "drilled", op: { type: "Difference", left: 1, right: 3 } },
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
    roots: [{ root: 4, material: "default" }],
    parameters: {
      hole_r: { value: holeR, unit: "mm" },
      plate_t: { value: plateT, unit: "mm" },
    },
    bindings: { "2:radius": "hole_r", "1:size.z": "plate_t" },
  } as unknown as Document;
}

describe("sensitivity tool", () => {
  let engine: Engine;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    documents.clear();
  });

  it("ranks parameters by influence and reports units and routes", () => {
    const out = json(
      sensitivity(
        {
          document: plateDoc(4, 10),
          quantities: ["volume"],
          find_trust_radius: false,
        },
        engine,
      ),
    );
    const rows = out.rows as Array<Record<string, unknown>>;
    expect(rows.length).toBe(2);
    for (const row of rows) {
      expect(row.unit).toBe("mm^3/mm");
      // volume is an exact seam derivative
      expect((row.route as Record<string, unknown>).route).toBe("dual");
      expect(row.basis).toBe("verified");
      expect(row.verdict).toBe("pass");
    }
    // Thickening the plate adds volume; widening the hole removes it.
    const byName = Object.fromEntries(rows.map((r) => [r.parameter, r.value as number]));
    expect(byName.plate_t).toBeGreaterThan(0);
    expect(byName.hole_r).toBeLessThan(0);
    expect(out.allUsable).toBe(true);
    expect(String(out.rendered)).toContain("d(volume)/d(parameter)");
  });

  it("searches out the real topology boundary as a trust radius", () => {
    const out = json(
      sensitivity(
        {
          document: plateDoc(7, 10),
          parameters: ["hole_r"],
          quantities: ["volume"],
          find_trust_radius: true,
        },
        engine,
      ),
    );
    const rows = out.rows as Array<Record<string, unknown>>;
    const trust = rows[0].trust as Record<string, unknown>;
    expect(trust).toBeTruthy();
    expect(trust.limited_by).toBe("topology_stable");
    // The hole becomes tangent to the 20 mm plate's side faces at r = 10.
    expect(trust.upper as number).toBeGreaterThan(9.5);
    expect(trust.upper as number).toBeLessThanOrEqual(10.2);
    expect(trust.lower as number).toBeLessThan(7);
  });

  it("bbox rows are finite differences and may never claim verified", () => {
    const out = json(
      sensitivity(
        {
          document: plateDoc(4, 10),
          parameters: ["plate_t"],
          quantities: ["bbox_z"],
          find_trust_radius: false,
        },
        engine,
      ),
    );
    const row = (out.rows as Array<Record<string, unknown>>)[0];
    expect((row.route as Record<string, unknown>).route).toBe("finite_difference");
    expect(row.basis).toBe("predicted");
    // The plate's z extent *is* the thickness, so this derivative is 1.
    expect(row.value as number).toBeCloseTo(1, 2);
  });

  it("emits one receipt claim per row", () => {
    const out = json(
      sensitivity(
        {
          document: plateDoc(4, 10),
          quantities: ["volume", "mass"],
          find_trust_radius: false,
        },
        engine,
      ),
    );
    const claims = out.claims as Array<Record<string, unknown>>;
    expect(claims.length).toBe(4); // 2 parameters x 2 quantities
    for (const c of claims) {
      expect(c.domain).toBe("sensitivity");
      expect(String(c.id)).toMatch(/^sensitivity\//);
      expect(c.verdict).toBe("pass");
    }
  });

  it("rejects an unknown quantity by name", () => {
    expect(() =>
      sensitivity(
        { document: plateDoc(4, 10), quantities: ["wobble"] },
        engine,
      ),
    ).toThrow(/unknown quantity/i);
  });
});
