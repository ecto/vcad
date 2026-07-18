/**
 * Regression tests for the torr-session field-report fixes:
 *
 *  1. Unknown tool arguments are rejected at dispatch (never silently
 *     ignored), and render_view's `style` accepts only known styles.
 *  2. Assembly-only documents (partDefs + instances, no scene roots) are
 *     first-class through `measure` and mutation integrity reports —
 *     previously `measure` answered "Available: none" and create_cad_loon
 *     reported `parts: 0, volume: 0` for a successful assembly.
 *  3. `check_clearance` distinguishes touching from intersecting, and
 *     `allow_contact` lets designed-contact pairs pass.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { Document } from "@vcad/ir";
import { createServer } from "../server.js";
import { documents } from "../tools/session.js";
import { measureResult, MeasureError } from "../tools/measure.js";
import { computeIntegrity } from "../tools/integrity.js";
import {
  clearanceVerdict,
  clearanceHolds,
  computeGroupClearance,
} from "../tools/clearance.js";
import { unknownArgKeys } from "../tools/validate-args.js";
import { renderView } from "../tools/render.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

const ASSEMBLY_LOON = `
[let base [cube 40 40 5]]
[let post [cylinder 5 30]]
[assembly
  #[[part "base" base "aluminum"]
    [part "post" post "steel"]]
  #[[instance "base1" "base" 0 0 0]
    [instance "post1" "post" 20 20 5]]
  #[]
  "base1"]
`;

function assemblyDoc(): Document {
  const doc = engine.evalVcadSource(ASSEMBLY_LOON);
  if (!doc) throw new Error("loon eval failed");
  return doc;
}

describe("strict tool arguments", () => {
  it("unknownArgKeys flags undeclared keys and honors additionalProperties", () => {
    const schema = {
      type: "object",
      properties: { document_id: { type: "string" } },
    };
    expect(unknownArgKeys(schema, { document_id: "d", style: "raytrace" })).toEqual([
      "style",
    ]);
    expect(unknownArgKeys(schema, { document_id: "d" })).toEqual([]);
    expect(
      unknownArgKeys(
        { ...schema, additionalProperties: true },
        { anything: 1 },
      ),
    ).toEqual([]);
    expect(unknownArgKeys(undefined, { anything: 1 })).toEqual([]);
  });

  it("dispatch rejects an unknown argument with a structured error", async () => {
    documents.clear();
    const server = await createServer(engine, { user: null });
    const [clientT, serverT] = InMemoryTransport.createLinkedPair();
    const client = new Client(
      { name: "strict-args", version: "0.0.0" },
      { capabilities: {} },
    );
    await Promise.all([client.connect(clientT), server.connect(serverT)]);
    const result = (await client.callTool({
      name: "inspect_cad",
      arguments: { document_id: "nonexistent", raytrace: true },
    })) as { isError?: boolean; content: Array<{ type: string; text?: string }> };
    expect(result.isError).toBe(true);
    const body = JSON.parse(result.content[0]!.text!);
    expect(body.error).toContain("'raytrace'");
    expect(body.accepted_arguments).toContain("document_id");
    await client.close();
  });

  it("render_view rejects an unknown style value loudly", async () => {
    const result = await renderView({
      document: { version: "0.1", nodes: {}, roots: [] },
      style: "raytrace",
    });
    expect(result.isError).toBe(true);
    const body = JSON.parse(
      (result.content[0] as { type: "text"; text: string }).text,
    );
    expect(body.error).toContain("unknown style 'raytrace'");
    expect(body.error).toContain("shaded");
  });
});

describe("assembly-only documents through measure + integrity", () => {
  it("measure resolves assembly instance ids and names", () => {
    const doc = assemblyDoc();
    const pair = measureResult(doc, engine, ["base1", "post1"]);
    expect(pair.mode).toBe("pair");
    // The post sits on top of the base plate — contact, not clearance.
    expect(pair.distance_mm as number).toBeLessThanOrEqual(0.001);
    const single = measureResult(doc, engine, ["post1"]) as {
      part: { volume_mm3: number };
    };
    // World-placed cylinder r=5 h=30 → ~2356 mm³ (tessellation-bound).
    expect(single.part.volume_mm3).toBeGreaterThan(2000);
  });

  it("measure error for a bad id lists instance candidates, not 'none'", () => {
    const doc = assemblyDoc();
    try {
      measureResult(doc, engine, ["nope"]);
      expect.unreachable("expected MeasureError");
    } catch (e) {
      expect(e).toBeInstanceOf(MeasureError);
      expect((e as Error).message).toContain("base1");
      expect((e as Error).message).not.toContain("Available: none");
    }
  });

  it("computeIntegrity reports instance geometry instead of parts:0 volume:0", () => {
    const doc = assemblyDoc();
    const report = computeIntegrity(doc, engine);
    expect(report).not.toBeNull();
    expect(report!.instances).toBe(2);
    // base 40*40*5 = 8000 plus the post ≈ 2356.
    expect(report!.volume_mm3).toBeGreaterThan(8000);
    expect(report!.bounding_box).not.toBeNull();
    expect(report!.bounding_box!.max.z).toBeGreaterThan(30);
  });
});

describe("check_clearance touching verdict + allow_contact", () => {
  it("clearanceVerdict classifies clear / touching / intersecting", () => {
    expect(clearanceVerdict(1.5)).toBe("clear");
    expect(clearanceVerdict(0)).toBe("touching");
    expect(clearanceVerdict(0.0005)).toBe("touching");
    expect(clearanceVerdict(-0.0005)).toBe("touching");
    expect(clearanceVerdict(-0.5)).toBe("intersecting");
  });

  it("clearanceHolds passes touching pairs only when contact is allowed", () => {
    expect(clearanceHolds(0, 0.5, false)).toBe(false);
    expect(clearanceHolds(0, 0.5, true)).toBe(true);
    expect(clearanceHolds(-0.5, 0.5, true)).toBe(false);
    expect(clearanceHolds(1.0, 0.5, false)).toBe(true);
  });

  it("a bolted-flush assembly pair measures as touching, not intersecting", () => {
    const doc = assemblyDoc();
    const { result, error } = computeGroupClearance(
      doc,
      engine,
      ["base1"],
      ["post1"],
    );
    expect(error).toBeUndefined();
    expect(result).toBeDefined();
    expect(clearanceVerdict(result!.distance_mm)).toBe("touching");
  });
});
