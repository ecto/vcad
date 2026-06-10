import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine, getKernelWasm } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { commandRegistry } from "@vcad/core";
import { exportCad } from "../tools/export.js";
import { inspectCad } from "../tools/inspect.js";
import {
  sheetMetalCreate,
  sheetMetalUnfold,
  sheetMetalCheck,
  sheetMetalMaterials,
  sheetMetalBendTable,
  sheetMetalCost,
  sheetMetalSuggestFix,
  sheetMetalSequence,
  sheetMetalNest,
} from "../tools/sheet-metal.js";
import {
  openDocument,
  getDocumentTool,
  closeDocument,
  documents,
} from "../tools/session.js";
import {
  registryDispatchableNames,
  registryToolDescriptors,
  dispatchRegistryTool,
} from "../tools/registry-dispatch.js";
import {
  createRobotEnv,
  gymStep,
  gymReset,
  gymObserve,
  gymClose,
} from "../tools/gym.js";
import {
  createSchematic,
  placeComponents,
  routeNets,
} from "../tools/ecad.js";
import { existsSync, unlinkSync } from "node:fs";
import { resolve } from "node:path";

/** Minimal Document with one cube part — replaces what createCadDocument
 *  used to build for downstream tests. */
function makeCubeDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "test_cube",
        op: { type: "Cube", size: { x: 10, y: 10, z: 10 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  };
}

/** Wire the kernel WASM into the registry. Required for `planCrud` to
 *  work — same bootstrap createServer does at startup. */
async function bootstrapRegistry(): Promise<void> {
  const wasm = (await getKernelWasm()) as unknown as Record<string, unknown>;
  const getToolSchemas = wasm.get_tool_schemas as (() => string) | undefined;
  if (getToolSchemas) commandRegistry.loadSchemas(getToolSchemas());
  const getAnthropicToolsJson = wasm.get_anthropic_tools_json as
    | (() => string)
    | undefined;
  const buildChatSystemPrompt = wasm.build_chat_system_prompt as
    | ((parts: string, sel: string) => string)
    | undefined;
  const planChatTool = wasm.plan_chat_tool as
    | ((tool: string, args: string, doc: string) => string)
    | undefined;
  if (getAnthropicToolsJson && buildChatSystemPrompt) {
    commandRegistry.setWasm({
      get_anthropic_tools_json: getAnthropicToolsJson,
      build_chat_system_prompt: buildChatSystemPrompt,
      plan_chat_tool: planChatTool,
    });
  }
}

beforeAll(async () => {
  await Engine.init();
  await bootstrapRegistry();
});

beforeEach(() => {
  documents.clear();
});

describe("session lifecycle", () => {
  it("opens, gets, and closes a fresh document", () => {
    const open = openDocument({});
    const { document_id } = JSON.parse(open.content[0].text);
    expect(document_id).toMatch(/^doc_/);
    expect(documents.has(document_id)).toBe(true);

    const get = getDocumentTool({ document_id });
    const doc = JSON.parse(get.content[0].text) as Document;
    expect(doc.version).toBe("0.1");
    expect(doc.roots).toHaveLength(0);

    const close = closeDocument({ document_id });
    expect(JSON.parse(close.content[0].text).closed).toBe(true);
    expect(documents.has(document_id)).toBe(false);
  });

  it("opens a session seeded with an existing IR", () => {
    const seeded = makeCubeDoc();
    const open = openDocument({ initial: seeded });
    const { document_id } = JSON.parse(open.content[0].text);

    const fetched = JSON.parse(
      getDocumentTool({ document_id }).content[0].text,
    ) as Document;
    expect(fetched.roots).toHaveLength(1);
    expect(Object.keys(fetched.nodes)).toContain("1");

    // Defensive copy — mutating the seed should NOT affect the session.
    seeded.roots.push({ root: 999, material: "default" });
    const after = JSON.parse(
      getDocumentTool({ document_id }).content[0].text,
    ) as Document;
    expect(after.roots).toHaveLength(1);
  });

  it("close_document on unknown id reports closed: false", () => {
    const out = closeDocument({ document_id: "doc_missing" });
    expect(JSON.parse(out.content[0].text).closed).toBe(false);
  });
});

describe("registry-driven tool surface", () => {
  it("exposes the kernel CRUD tools and filters out browser-only ones", () => {
    const names = registryDispatchableNames();
    // CRUD core surface is present.
    expect(names.has("create")).toBe(true);
    expect(names.has("read")).toBe(true);
    expect(names.has("update")).toBe(true);
    expect(names.has("delete")).toBe(true);
    expect(names.has("set_material")).toBe(true);
    // Browser-only tools must NOT be exposed via MCP.
    expect(names.has("focus_part")).toBe(false);
    expect(names.has("frame_all")).toBe(false);
    expect(names.has("set_view")).toBe(false);
  });

  it("each descriptor adds a required document_id to its inputSchema", () => {
    const descriptors = registryToolDescriptors();
    expect(descriptors.length).toBeGreaterThan(0);
    for (const d of descriptors) {
      const schema = d.inputSchema as { required?: string[]; properties?: Record<string, unknown> };
      expect(schema.required).toContain("document_id");
      expect(schema.properties).toHaveProperty("document_id");
    }
  });

  it("read on an empty session returns parts: []", () => {
    const { document_id } = JSON.parse(openDocument({}).content[0].text);
    const out = dispatchRegistryTool("read", { document_id });
    const payload = JSON.parse(out.content[0].text);
    expect(payload.parts).toEqual([]);
  });

  it("delete + set_material round-trip on a seeded document", () => {
    const { document_id } = JSON.parse(
      openDocument({ initial: makeCubeDoc() }).content[0].text,
    );
    // The seeded doc has a single root with id "1" — that's our part_id.
    const setOut = dispatchRegistryTool("set_material", {
      document_id,
      part_id: "1",
      material: "aluminum",
    });
    expect(setOut.content[0].text).toContain("aluminum");

    const after = JSON.parse(
      getDocumentTool({ document_id }).content[0].text,
    ) as Document;
    expect(after.roots[0].material).toBe("aluminum");
    expect(after.part_materials["1"]).toBe("aluminum");

    dispatchRegistryTool("delete", { document_id, part_id: "1" });
    const final = JSON.parse(
      getDocumentTool({ document_id }).content[0].text,
    ) as Document;
    expect(final.roots).toHaveLength(0);
    expect(final.nodes).toEqual({});
  });
});

describe("inspect_cad (session-aware)", () => {
  let engine: Engine;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("inspects a session document with a single cube", () => {
    const { document_id } = JSON.parse(
      openDocument({ initial: makeCubeDoc() }).content[0].text,
    );
    const result = inspectCad({ document_id }, engine);
    const props = JSON.parse(result.content[0].text);

    expect(props.volume_mm3).toBeCloseTo(1000, 0);
    expect(props.surface_area_mm2).toBeCloseTo(600, 0);
    expect(props.triangles).toBeGreaterThan(0);
    expect(props.parts).toBe(1);
  });
});

describe("export_cad", () => {
  let engine: Engine;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("exports a hand-built doc to STL", () => {
    const filename = "test_export.stl";
    const filepath = resolve(process.cwd(), filename);
    if (existsSync(filepath)) unlinkSync(filepath);

    const result = exportCad({ ir: makeCubeDoc(), filename }, engine);
    const output = JSON.parse(result.content[0].text);

    expect(output.format).toBe("stl");
    expect(output.bytes).toBeGreaterThan(84);
    expect(existsSync(filepath)).toBe(true);
    unlinkSync(filepath);
  });

  it("exports a hand-built doc to GLB", () => {
    const filename = "test_export.glb";
    const filepath = resolve(process.cwd(), filename);
    if (existsSync(filepath)) unlinkSync(filepath);

    const result = exportCad({ ir: makeCubeDoc(), filename }, engine);
    const output = JSON.parse(result.content[0].text);

    expect(output.format).toBe("glb");
    expect(output.bytes).toBeGreaterThan(12);
    expect(existsSync(filepath)).toBe(true);
    unlinkSync(filepath);
  });
});

describe("gym tools", () => {
  const robotDoc = {
    version: "0.1",
    nodes: {
      "1": { id: 1, name: "base", op: { type: "Cube", size: { x: 100, y: 100, z: 50 } } },
      "2": { id: 2, name: "link1", op: { type: "Cube", size: { x: 20, y: 20, z: 100 } } },
    },
    materials: {},
    roots: [{ root: 1, material: "default" }],
    part_materials: {},
    partDefs: {
      base: { id: "base", name: "Base", root: 1, defaultMaterial: null },
      link1: { id: "link1", name: "Link 1", root: 2, defaultMaterial: null },
    },
    instances: [
      { id: "base_inst", partDefId: "base", name: "Base", transform: null, material: null },
      { id: "link1_inst", partDefId: "link1", name: "Link 1", transform: null, material: null },
    ],
    joints: [
      {
        id: "joint1",
        name: "Joint 1",
        parentInstanceId: "base_inst",
        childInstanceId: "link1_inst",
        parentAnchor: { x: 0, y: 0, z: 25 },
        childAnchor: { x: 0, y: 0, z: -50 },
        kind: { type: "Revolute", axis: { x: 0, y: 1, z: 0 }, limits: [-90, 90] },
        state: 0,
      },
    ],
    groundInstanceId: "base_inst",
  };

  async function createEnvOrSkip(): Promise<string | null> {
    const result = await createRobotEnv({
      document: robotDoc,
      end_effector_ids: ["link1_inst"],
    });
    const info = JSON.parse(result.content[0].text);
    if (info.error) return null;
    return info.env_id;
  }

  it("creates robot environment (or reports physics unavailable)", async () => {
    const result = await createRobotEnv({
      document: robotDoc,
      end_effector_ids: ["link1_inst"],
    });

    const info = JSON.parse(result.content[0].text);

    if (info.error) {
      expect(info.error).toContain("physics");
    } else {
      expect(info.env_id).toBeDefined();
      expect(info.num_joints).toBeGreaterThanOrEqual(0);
      expect(info.end_effector_ids).toContain("link1_inst");
      gymClose({ env_id: info.env_id });
    }
  });

  it("steps with position control", async () => {
    const envId = await createEnvOrSkip();
    if (!envId) return;

    const stepResult = gymStep({
      env_id: envId,
      action_type: "position",
      values: [45],
    });

    const step = JSON.parse(stepResult.content[0].text);
    expect(step.observation).toBeDefined();
    expect(step.observation.joint_positions).toBeDefined();
    expect(step.reward).toBeDefined();
    expect(step.done).toBeDefined();

    gymClose({ env_id: envId });
  });

  it("resets environment", async () => {
    const envId = await createEnvOrSkip();
    if (!envId) return;

    gymStep({ env_id: envId, action_type: "position", values: [30] });
    const resetResult = gymReset({ env_id: envId });
    const obs = JSON.parse(resetResult.content[0].text);
    expect(obs.joint_positions).toBeDefined();
    gymClose({ env_id: envId });
  });

  it("observes without stepping", async () => {
    const envId = await createEnvOrSkip();
    if (!envId) return;

    gymStep({ env_id: envId, action_type: "position", values: [60] });
    const observeResult = gymObserve({ env_id: envId });
    const obs = JSON.parse(observeResult.content[0].text);

    expect(obs.joint_positions).toBeDefined();
    expect(obs.joint_velocities).toBeDefined();
    expect(obs.end_effector_poses).toBeDefined();
    gymClose({ env_id: envId });
  });

  it("closes environment", async () => {
    const envId = await createEnvOrSkip();
    if (!envId) return;

    const closeResult = gymClose({ env_id: envId });
    const closeInfo = JSON.parse(closeResult.content[0].text);
    expect(closeInfo.success).toBe(true);

    const closeAgain = gymClose({ env_id: envId });
    const errorInfo = JSON.parse(closeAgain.content[0].text);
    expect(errorInfo.error).toBeDefined();
  });
});

describe("sheet-metal tools", () => {
  let engine: Engine;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  it("create → unfold → check closes the loop", () => {
    // 100×50 base with one 25 mm flange off edge 0.
    const created = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 1,
          material: "Al-soft",
          flanges: [{ edge_index: 0, length: 25 }],
        },
        engine,
      ).content[0].text,
    );
    expect(created.document_id).toBeDefined();
    expect(created.model.panel_count).toBe(2);
    expect(created.model.bend_count).toBe(1);
    expect(created.violations).toHaveLength(0); // shop-ready vs. generic

    const unfolded = JSON.parse(
      sheetMetalUnfold(
        { document_id: created.document_id },
        engine,
      ).content[0].text,
    );
    expect(unfolded.flat_pattern.panel_outlines_2d).toHaveLength(2);
    expect(unfolded.dxf).toContain("0\nLAYER\n2\nCUT\n");
    expect(unfolded.dxf.trimEnd().endsWith("0\nEOF")).toBe(true);

    // Generic shop: clean. Strict shop (R/t ≥ 4): the 1 mm radius fails.
    const lenient = JSON.parse(
      sheetMetalCheck(
        { document_id: created.document_id },
        engine,
      ).content[0].text,
    );
    expect(lenient.shop_ready).toBe(true);

    const strict = JSON.parse(
      sheetMetalCheck(
        {
          document_id: created.document_id,
          shop_profile: { name: "Strict Inc", min_bend_radius_ratio: 4 },
        },
        engine,
      ).content[0].text,
    );
    expect(strict.shop_ready).toBe(false);
    expect(strict.shop.name).toBe("Strict Inc");
    expect(
      strict.violations.some(
        (v: { detail: { kind: string } }) =>
          v.detail.kind === "BendRadiusBelowMinimum",
      ),
    ).toBe(true);
  });

  it("unfold on an unknown document id throws", () => {
    expect(() =>
      sheetMetalUnfold({ document_id: "nope" }, engine),
    ).toThrow(/Unknown document_id/);
  });

  it("materials registry includes the six shop basics", () => {
    const out = JSON.parse(
      sheetMetalMaterials({}, engine).content[0].text,
    );
    const names: string[] = out.materials.map(
      (m: { name: string }) => m.name,
    );
    for (const expected of [
      "al-soft",
      "al-hard",
      "steel-mild",
      "ss-304",
      "brass",
      "copper",
    ]) {
      expect(names).toContain(expected);
    }
  });

  it("bend table returns curated rows", () => {
    const out = JSON.parse(
      sheetMetalBendTable({}, engine).content[0].text,
    );
    expect(out.table.id).toBe("builtin");
    expect(out.table.rows.length).toBeGreaterThan(10);
  });

  it("cost: breakdown drops per-unit setup at higher quantity", () => {
    const { document_id } = JSON.parse(
      sheetMetalCreate(
        {
          width: 200,
          depth: 100,
          thickness: 1.5,
          material: "steel-mild",
          flanges: [{ edge_index: 0, length: 30 }],
        },
        engine,
      ).content[0].text,
    );
    const one = JSON.parse(
      sheetMetalCost({ document_id, quantity: 1 }, engine).content[0].text,
    );
    const hundred = JSON.parse(
      sheetMetalCost({ document_id, quantity: 100 }, engine).content[0].text,
    );
    expect(one.breakdown.total_each).toBeGreaterThan(0);
    expect(one.breakdown.currency).toBe("USD");
    // Setup amortizes — per-part total drops with volume.
    expect(hundred.breakdown.total_each).toBeLessThan(one.breakdown.total_each);
    // Materials change cost: density of steel-mild routes through registry.
    expect(one.breakdown.mass_kg_each).toBeGreaterThan(0.1);
  });

  it("cost: custom rates override defaults", () => {
    const { document_id } = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 1,
          material: "al-soft",
          flanges: [{ edge_index: 0, length: 20 }],
        },
        engine,
      ).content[0].text,
    );
    const cheap = JSON.parse(
      sheetMetalCost(
        { document_id, rates: { markup_pct: 0 } },
        engine,
      ).content[0].text,
    );
    const dear = JSON.parse(
      sheetMetalCost(
        { document_id, rates: { markup_pct: 200 } },
        engine,
      ).content[0].text,
    );
    expect(dear.breakdown.total_each).toBeGreaterThan(cheap.breakdown.total_each);
    expect(cheap.rates.markup_pct).toBe(0);
    // Field-tolerance: other rates fell back to generic.
    expect(cheap.rates.currency).toBe("USD");
  });

  it("hem: closed hem creates an extra panel + 180° crease", () => {
    const out = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 1,
          material: "al-soft",
          hems: [{ edge_index: 0, length: 6 }],
        },
        engine,
      ).content[0].text,
    );
    expect(out.model.panel_count).toBe(2);
    expect(out.model.bend_count).toBe(1);
    // 180° fold ≈ π radians.
    expect(out.model.bends[0].angle_rad).toBeCloseTo(Math.PI, 5);
    // Provenance carries the hem tag.
    expect(out.model.bends[0].k_factor_source).toContain(";hem:closed");

    const unfolded = JSON.parse(
      sheetMetalUnfold({ document_id: out.document_id }, engine).content[0]
        .text,
    );
    expect(unfolded.flat_pattern.creases).toHaveLength(1);
    expect(unfolded.dxf).toContain("0\nLINE\n8\nBEND_UP");
  });

  it("material-aware check: al-hard flags R/t=1 that al-soft passes", () => {
    // R=1 mm on 1 mm stock: R/t = 1.
    // al-soft min R/t = 0 → shop-ready; al-hard min R/t = 1.5 → flagged.
    const soft = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 1,
          material: "al-soft",
          flanges: [{ edge_index: 0, length: 25, radius: 1 }],
        },
        engine,
      ).content[0].text,
    );
    const softCheck = JSON.parse(
      sheetMetalCheck({ document_id: soft.document_id }, engine).content[0]
        .text,
    );
    expect(softCheck.shop_ready).toBe(true);

    const hard = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 1,
          material: "al-hard",
          flanges: [{ edge_index: 0, length: 25, radius: 1 }],
        },
        engine,
      ).content[0].text,
    );
    const hardCheck = JSON.parse(
      sheetMetalCheck({ document_id: hard.document_id }, engine).content[0]
        .text,
    );
    expect(hardCheck.shop_ready).toBe(false);
    const radiusViol = hardCheck.violations.find(
      (v: { detail: { kind: string; source?: string } }) =>
        v.detail.kind === "BendRadiusBelowMinimum",
    );
    expect(radiusViol).toBeDefined();
    expect(radiusViol.detail.source).toBe("Material");
    expect(radiusViol.detail.material).toBe("al-hard");
  });

  it("nesting: explicit footprints pack onto stock sheets", () => {
    const out = JSON.parse(
      sheetMetalNest(
        {
          parts: [
            { name: "A", width_mm: 100, height_mm: 50, quantity: 4 },
            { name: "B", width_mm: 80, height_mm: 80, quantity: 2 },
          ],
          params: {
            stock_width_mm: 1000,
            stock_height_mm: 500,
            spacing_mm: 5,
            edge_margin_mm: 10,
            allow_rotation: true,
          },
        },
        engine,
      ).content[0].text,
    );
    expect(out.result.placements).toHaveLength(6);
    expect(out.result.sheets_used).toBe(1);
    expect(out.result.unplaceable).toEqual([]);
    expect(out.result.utilization_pct).toBeGreaterThan(0);
  });

  it("nesting: footprint from session document_id", () => {
    const { document_id } = JSON.parse(
      sheetMetalCreate(
        {
          width: 200,
          depth: 100,
          thickness: 1,
          material: "al-soft",
        },
        engine,
      ).content[0].text,
    );
    const out = JSON.parse(
      sheetMetalNest(
        {
          parts: [{ document_id, quantity: 3 }],
        },
        engine,
      ).content[0].text,
    );
    expect(out.result.placements).toHaveLength(3);
    expect(out.parts[0].width_mm).toBeCloseTo(200, 1);
    expect(out.parts[0].height_mm).toBeCloseTo(100, 1);
  });

  it("bend sequence: outermost bends form first", () => {
    // U-channel with a hem on one of the sides. Hem (depth 2) should
    // sort ahead of the parent flanges (depth 1).
    const { document_id } = JSON.parse(
      sheetMetalCreate(
        {
          width: 80,
          depth: 40,
          thickness: 1,
          material: "al-soft",
          flanges: [
            { edge_index: 0, length: 20 },
            { edge_index: 2, length: 20 },
          ],
          hems: [{ edge_index: 2, length: 5, panel_id: 1 }],
        },
        engine,
      ).content[0].text,
    );
    const out = JSON.parse(
      sheetMetalSequence({ document_id }, engine).content[0].text,
    );
    expect(out.count).toBe(3);
    expect(out.steps[0].depth).toBeGreaterThanOrEqual(out.steps[1].depth);
    expect(out.steps[1].depth).toBeGreaterThanOrEqual(out.steps[2].depth);
  });

  it("base flange from arbitrary outline: L-bracket polygon", () => {
    // L-shaped base flange.
    const out = JSON.parse(
      sheetMetalCreate(
        {
          outline: [
            [0, 0],
            [50, 0],
            [50, 15],
            [20, 15],
            [20, 50],
            [0, 50],
          ],
          thickness: 1,
          material: "al-soft",
        },
        engine,
      ).content[0].text,
    );
    expect(out.model.panel_count).toBe(1);
    expect(out.model.bend_count).toBe(0);
    // Flat area equals the L-shape area: 50×15 + 20×35 = 1450 mm².
    expect(out.flat.area_mm2).toBeCloseTo(1450, 0);
  });

  it("base flange polygon: holes propagate to the flat pattern", () => {
    const create = JSON.parse(
      sheetMetalCreate(
        {
          outline: [
            { x: 0, y: 0 },
            { x: 40, y: 0 },
            { x: 40, y: 20 },
            { x: 0, y: 20 },
          ],
          holes: [
            [
              [10, 5],
              [10, 15],
              [15, 15],
              [15, 5],
            ],
          ],
          thickness: 1,
          material: "al-soft",
        },
        engine,
      ).content[0].text,
    );
    const unfolded = JSON.parse(
      sheetMetalUnfold(
        { document_id: create.document_id },
        engine,
      ).content[0].text,
    );
    expect(unfolded.flat_pattern.panel_holes_2d[0]).toHaveLength(1);
    expect(unfolded.dxf).toContain("0\nLAYER\n2\nCUT\n");
    // Two LWPOLYLINE entries on CUT: 1 outline + 1 hole.
    expect(unfolded.dxf.match(/0\nLWPOLYLINE\n8\nCUT/g)?.length).toBe(2);
  });

  it("jog: creates a Z-shaped offset with two opposite 90° bends", () => {
    const out = JSON.parse(
      sheetMetalCreate(
        {
          width: 120,
          depth: 60,
          thickness: 1,
          material: "al-soft",
          jogs: [{ edge_index: 0, offset: 5, length: 25 }],
        },
        engine,
      ).content[0].text,
    );
    expect(out.model.panel_count).toBe(3);
    expect(out.model.bend_count).toBe(2);
    expect(out.model.bends[0].angle_rad).toBeCloseTo(Math.PI / 2, 5);
    expect(out.model.bends[1].angle_rad).toBeCloseTo(Math.PI / 2, 5);
    expect(out.model.bends[0].direction).not.toBe(out.model.bends[1].direction);
    expect(out.model.bends[0].k_factor_source).toContain(";jog:a");
    expect(out.model.bends[1].k_factor_source).toContain(";jog:b");
  });

  it("springback: bend summary includes compensated angle per material", () => {
    // Al-hard has higher springback than al-soft.
    const soft = JSON.parse(
      sheetMetalCreate(
        {
          width: 80,
          depth: 40,
          thickness: 1,
          material: "al-soft",
          flanges: [{ edge_index: 0, length: 15, radius: 2 }],
        },
        engine,
      ).content[0].text,
    );
    const hard = JSON.parse(
      sheetMetalCreate(
        {
          width: 80,
          depth: 40,
          thickness: 1,
          material: "al-hard",
          flanges: [{ edge_index: 0, length: 15, radius: 2 }],
        },
        engine,
      ).content[0].text,
    );
    const softBend = soft.model.bends[0];
    const hardBend = hard.model.bends[0];
    expect(softBend.springback_rad).toBeGreaterThan(0);
    expect(hardBend.springback_rad).toBeGreaterThan(softBend.springback_rad);
    expect(softBend.compensated_angle_rad).toBeCloseTo(
      softBend.angle_rad + softBend.springback_rad,
      6,
    );
  });

  it("suggest_fix: maps violations to actionable parameter changes", () => {
    // Short flange flags FlangeBelowMinHeight → suggest increasing length.
    const { document_id } = JSON.parse(
      sheetMetalCreate(
        {
          width: 100,
          depth: 50,
          thickness: 1,
          material: "al-soft",
          flanges: [{ edge_index: 0, length: 2 }],
        },
        engine,
      ).content[0].text,
    );
    const out = JSON.parse(
      sheetMetalSuggestFix({ document_id }, engine).content[0].text,
    );
    expect(out.count).toBeGreaterThan(0);
    const flangeFix = out.suggestions.find(
      (s: { fix: { action: string } }) =>
        s.fix.action === "increase_flange_length",
    );
    expect(flangeFix).toBeDefined();
    expect(flangeFix.fix.new_length_mm).toBeGreaterThanOrEqual(5);
    expect(flangeFix.fix.description.toLowerCase()).toContain("flange");
  });
});

describe("ecad place_components → route_nets pipeline", () => {
  it("assigns pad nets during placement so routing produces traces", async () => {
    const schematicOut = JSON.parse(
      createSchematic({
        components: [
          {
            ref: "R1",
            value: "10k",
            footprint: "Resistor_SMD:R_0805",
            x: 0,
            y: 0,
            pins: [
              { number: "1", name: "VCC", type: "Passive" },
              { number: "2", name: "OUT", type: "Passive" },
            ],
          },
          {
            ref: "R2",
            value: "4k7",
            footprint: "Resistor_SMD:R_0805",
            x: 20,
            y: 0,
            pins: [
              { number: "1", name: "OUT", type: "Passive" },
              { number: "2", name: "GND", type: "Passive" },
            ],
          },
        ],
      }).content[0].text,
    );

    const placedOut = JSON.parse(
      (
        await placeComponents({
          document: schematicOut.document,
          board_width: 50,
          board_height: 50,
        })
      ).content[0].text,
    );
    expect(placedOut.success).toBe(true);
    expect(placedOut.footprints_placed).toBe(2);

    const placedDoc = placedOut.document as Document;
    const pcbNode = Object.values(placedDoc.nodes).find(
      (n) => (n.op as { type: string }).type === "PcbBoard",
    );
    expect(pcbNode).toBeDefined();
    const board = (pcbNode!.op as unknown as { board: { footprints: Array<{ pads: Array<{ net?: string }> }> } }).board;
    const padNets = board.footprints.flatMap((fp) => fp.pads.map((p) => p.net));
    expect(padNets).toEqual(["VCC", "OUT", "OUT", "GND"]);

    const routedOut = JSON.parse(
      routeNets({ document: placedDoc }).content[0].text,
    );
    expect(routedOut.success).toBe(true);
    // Only OUT has 2+ pads — VCC and GND are single-pad nets.
    expect(routedOut.nets_routed).toBe(1);
    expect(routedOut.traces_added).toBe(1);
  });

  it("derives pad nets from wire connectivity, not pin names", async () => {
    // Two resistors with anonymous ("~") pins — connectivity must come from
    // the wires. R1 pin2 (5,0) — wire — R2 pin1 (15,0), labeled "MID".
    const schematicOut = JSON.parse(
      createSchematic({
        components: [
          {
            ref: "R1",
            value: "10k",
            footprint: "Resistor_SMD:R_0805",
            x: 0,
            y: 0,
            pins: [
              { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
              { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
            ],
          },
          {
            ref: "R2",
            value: "4k7",
            footprint: "Resistor_SMD:R_0805",
            x: 20,
            y: 0,
            pins: [
              { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
              { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
            ],
          },
        ],
        wires: [{ x1: 5, y1: 0, x2: 15, y2: 0 }],
        labels: [{ name: "MID", x: 5, y: 0 }],
      }).content[0].text,
    );

    const placedOut = JSON.parse(
      (
        await placeComponents({
          document: schematicOut.document,
          board_width: 50,
          board_height: 50,
        })
      ).content[0].text,
    );
    expect(placedOut.success).toBe(true);

    const placedDoc = placedOut.document as Document;
    const pcbNode = Object.values(placedDoc.nodes).find(
      (n) => (n.op as { type: string }).type === "PcbBoard",
    );
    const board = (
      pcbNode!.op as unknown as {
        board: { footprints: Array<{ ref: string; pads: Array<{ number: string; net?: string }> }> };
      }
    ).board;

    const padNet = (ref: string, num: string) =>
      board.footprints
        .find((fp) => fp.ref === ref)!
        .pads.find((p) => p.number === num)!.net;

    // Wired pins share the labeled net; unwired pins get no net.
    expect(padNet("R1", "2")).toBe("MID");
    expect(padNet("R2", "1")).toBe("MID");
    expect(padNet("R1", "1")).toBeUndefined();
    expect(padNet("R2", "2")).toBeUndefined();
  });

  it("keeps all footprints inside the board outline", async () => {
    const comps = Array.from({ length: 5 }, (_, i) => ({
      ref: `R${i + 1}`,
      value: "1k",
      footprint: "Resistor_SMD:R_0805",
      x: i * 10,
      y: 0,
      pins: [
        { number: "1", name: "A", type: "Passive" },
        { number: "2", name: "B", type: "Passive" },
      ],
    }));
    const schematicOut = JSON.parse(
      createSchematic({ components: comps }).content[0].text,
    );

    for (const strategy of ["grid", "force_directed"]) {
      const placedOut = JSON.parse(
        (
          await placeComponents({
            document: structuredClone(schematicOut.document),
            board_width: 25,
            board_height: 15,
            strategy,
          })
        ).content[0].text,
      );
      expect(placedOut.success).toBe(true);
      expect(placedOut.strategy).toBe(strategy);

      const pcbNode = Object.values(placedOut.document.nodes).find(
        (n) => ((n as { op: { type: string } }).op).type === "PcbBoard",
      ) as { op: { board: { footprints: Array<{ position: { x: number; y: number } }> } } };
      for (const fp of pcbNode.op.board.footprints) {
        expect(fp.position.x).toBeGreaterThan(0);
        expect(fp.position.x).toBeLessThan(25);
        expect(fp.position.y).toBeGreaterThan(0);
        expect(fp.position.y).toBeLessThan(15);
      }
    }
  });
});
