/**
 * v2 surface tests for the second wave of tools — assemble, parts,
 * render, drawing, simulate, ECAD.
 */

import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { buildTool } from "../tools/build.js";
import { assemble } from "../tools/assemble.js";
import { partsTool } from "../tools/parts-v2.js";
import { render } from "../tools/render.js";
import { drawing } from "../tools/drawing.js";
import { simulate } from "../tools/simulate.js";
import {
  schematicV2,
  layoutV2,
  routeV2,
  checkV2,
  gerberV2,
  calcImpedanceV2,
} from "../tools/ecad-v2.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

function envelope(result: { content: Array<{ type: string; [k: string]: unknown }> }): {
  ok: boolean;
  doc?: string;
  result?: unknown;
  stats?: { parts: number; nodes: number };
  error?: { code: string; message: string };
} {
  const text = result.content.find((c) => c.type === "text");
  if (!text) throw new Error("envelope missing text content");
  return JSON.parse((text as { text: string }).text);
}

function buildBlockHandle(): string {
  const r = buildTool(
    { ops: [{ op: "primitive", kind: "cube", size: { x: 10, y: 10, z: 5 }, name: "block" }] },
    engine,
  );
  return envelope(r).doc!;
}

describe("assemble (v2)", () => {
  it("adds an instance and a fixed joint", () => {
    const handle = buildBlockHandle();
    const r = assemble(
      {
        doc: handle,
        instances: [{ name: "a", part: "block" }, { name: "b", part: "block", transform: { translate: { x: 100, y: 0, z: 0 } } }],
        joints: [
          {
            name: "j",
            parent: "a",
            child: "b",
            kind: "fixed",
            anchor_parent: { x: 0, y: 0, z: 0 },
            anchor_child: { x: 0, y: 0, z: 0 },
          },
        ],
        ground: "a",
      },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const res = env.result as { added_instances: string[]; added_joints: string[]; ground: string };
    expect(res.added_instances).toEqual(["a", "b"]);
    expect(res.added_joints).toEqual(["j"]);
    expect(res.ground).toBe("a");
  });

  it("reports interference when instances overlap", () => {
    const handle = buildBlockHandle();
    const r = assemble(
      {
        doc: handle,
        instances: [
          { name: "a", part: "block" },
          { name: "b", part: "block", transform: { translate: { x: 5, y: 0, z: 0 } } },
        ],
      },
      engine,
    );
    const res = envelope(r).result as {
      interferences: Array<{ a: string; b: string; volume_estimate: number }>;
    };
    expect(res.interferences.length).toBeGreaterThan(0);
    expect(res.interferences[0].volume_estimate).toBeGreaterThan(0);
  });
});

describe("parts (v2)", () => {
  it("search returns the manifest contract shape", () => {
    const r = partsTool({ mode: "search", query: "" }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const res = env.result as { matches: unknown[]; count: number };
    expect(Array.isArray(res.matches)).toBe(true);
    expect(typeof res.count).toBe("number");
  });

  it("place rejects unknown ids", () => {
    const r = partsTool({ mode: "place", id: "std:nope.fake" }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("unknown_part");
  });
});

describe("render (v2)", () => {
  it("returns a base64 PNG resource at preview quality", () => {
    const handle = buildBlockHandle();
    const r = render({ doc: handle, quality: "preview" }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const res = env.result as {
      image: { kind: string; mime: string; data_base64: string };
      width: number;
      height: number;
      quality: string;
    };
    expect(res.image.mime).toBe("image/png");
    expect(res.image.data_base64.length).toBeGreaterThan(100);
    expect(res.width).toBe(512);
    expect(res.quality).toBe("preview");
  });

  it("returns raytracer_unavailable for high quality", () => {
    const handle = buildBlockHandle();
    const r = render({ doc: handle, quality: "high" }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("raytracer_unavailable");
  });
});

describe("drawing (v2)", () => {
  it("renders default 4-up sheet as SVG", () => {
    const handle = buildBlockHandle();
    const r = drawing({ doc: handle }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const res = env.result as { svg: string; views: string[] };
    expect(res.svg).toMatch(/^<svg/);
    expect(res.views.length).toBe(4);
    expect(res.svg).toContain("FRONT");
  });
});

describe("simulate (v2)", () => {
  it("rejects non-assembly docs", async () => {
    const handle = buildBlockHandle();
    const r = await simulate(
      { doc: handle, actions: [[0]], action_type: "torque" },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(false);
    expect(env.error?.code).toBe("not_an_assembly");
  });
});

describe("ECAD v2 wrappers", () => {
  it("schematic creates a doc handle with a schematic sheet", () => {
    const r = schematicV2(
      {
        title: "test",
        components: [
          {
            ref: "R1",
            value: "10k",
            footprint: "0805",
            x: 0,
            y: 0,
            pins: [{ number: "1", name: "1", x: 0, y: 0 }, { number: "2", name: "2", x: 5, y: 0 }],
          },
        ],
        wires: [],
        labels: [],
      },
      engine,
    );
    const env = envelope(r);
    expect(env.ok).toBe(true);
    expect(env.doc).toMatch(/^vcad:doc:/);
    const res = env.result as { components: number };
    expect(res.components).toBe(1);
  });

  it("calc_impedance returns a Z0 estimate", () => {
    const r = calcImpedanceV2({
      config: "microstrip",
      width: 0.2,
      height: 0.1,
      thickness: 0.035,
      er: 4.4,
    });
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const res = env.result as { Z0?: number; impedance?: number } | null;
    expect(res).toBeTruthy();
  });

  it("check unifies DRC + ERC even on a fresh doc", () => {
    const sch = schematicV2({ components: [], wires: [], labels: [] }, engine);
    const handle = envelope(sch).doc!;
    const r = checkV2({ doc: handle }, engine);
    const env = envelope(r);
    expect(env.ok).toBe(true);
    const res = env.result as { errors: unknown[]; warnings: unknown[]; info: unknown[] };
    expect(Array.isArray(res.errors)).toBe(true);
  });

  // layout/route/gerber require a complete schematic+pcb pipeline; they
  // are covered by the legacy tests via the v1 functions.
  void layoutV2;
  void routeV2;
  void gerberV2;
});
