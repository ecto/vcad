import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine, getKernelWasm } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { commandRegistry } from "@vcad/core";
import { exportCad } from "../tools/export.js";
import { inspectCad } from "../tools/inspect.js";
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
