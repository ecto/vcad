import { describe, it, expect, beforeAll, beforeEach, afterEach } from "vitest";
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
  saveDocument,
  loadDocument,
  documents,
} from "../tools/session.js";
import { InMemorySessionStore } from "../session-store.js";
import {
  registryDispatchableNames,
  registryToolDescriptors,
  dispatchRegistryTool,
} from "../tools/registry-dispatch.js";
import { getArtifactFile, clearArtifacts } from "../tools/artifact-store.js";
import { slimPreviewForInlineUi } from "../server.js";
import {
  createRobotEnv,
  gymStep,
  gymReset,
  gymObserve,
  gymClose,
} from "../tools/gym.js";
import { existsSync, unlinkSync, mkdtempSync, rmSync } from "node:fs";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";

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

describe("get_document returns the IR body (issue #278)", () => {
  // Regression: get_document is documented to "Return the full IR Document
  // JSON" but callers observed only a {document_id} stub — useless for
  // snapshotting board state or handing the document to another connection.

  let prevCap: string | undefined;

  beforeEach(() => {
    prevCap = process.env.MCP_MAX_INLINE_ARTIFACT_BYTES;
    clearArtifacts();
  });

  afterEach(() => {
    if (prevCap === undefined) delete process.env.MCP_MAX_INLINE_ARTIFACT_BYTES;
    else process.env.MCP_MAX_INLINE_ARTIFACT_BYTES = prevCap;
    clearArtifacts();
  });

  it("a small document comes back with its parts and nodes inline", () => {
    const open = openDocument({ initial: makeCubeDoc() });
    const { document_id } = JSON.parse(open.content[0].text);

    const result = getDocumentTool({ document_id });
    const doc = JSON.parse(result.content[0].text);

    // The IR body — not a {document_id} stub.
    expect(Object.keys(doc)).not.toEqual(["document_id"]);
    expect(doc.nodes["1"].op.type).toBe("Cube");
    expect(doc.roots).toHaveLength(1);
    expect(doc.version).toBe("0.1");
  });

  it("an oversized document offloads to the artifact store with a verifiable manifest", () => {
    // Tighten the inline cap so the offload branch triggers without building
    // a 64 KiB doc (remote.test.ts covers the default cap value).
    process.env.MCP_MAX_INLINE_ARTIFACT_BYTES = "2048";

    const big = makeCubeDoc();
    for (let i = 2; i <= 60; i++) {
      big.nodes[String(i)] = {
        id: i,
        name: `padding_cube_${i}`,
        op: { type: "Cube", size: { x: i, y: i, z: i } },
      };
      big.roots.push({ root: i, material: "default" });
    }
    expect(JSON.stringify(big).length).toBeGreaterThan(2048);

    const open = openDocument({ initial: big });
    const { document_id } = JSON.parse(open.content[0].text);

    const result = getDocumentTool({ document_id });
    const handle = JSON.parse(result.content[0].text);

    // Compact handle, not the IR — but with enough to act on.
    expect(handle.document_id).toBe(document_id);
    expect(handle.parts).toBe(60);
    expect(handle.nodes).toBe(60);
    expect(handle.artifact_id).toMatch(/^art_/);
    expect(handle.artifact_url).toContain(`/artifacts/${handle.artifact_id}`);
    expect(handle.manifest).toHaveLength(1);
    expect(handle.manifest[0].file).toBe(`${document_id}.vcad`);

    // The stored bytes ARE the full IR, and the manifest sha256 verifies them.
    const stored = getArtifactFile(handle.artifact_id, `${document_id}.vcad`);
    expect(stored).not.toBeNull();
    const sha = createHash("sha256").update(stored!.buf).digest("hex");
    expect(sha).toBe(handle.manifest[0].sha256);
    const roundTripped = JSON.parse(stored!.buf.toString("utf8")) as Document;
    expect(Object.keys(roundTripped.nodes)).toHaveLength(60);
    expect(roundTripped.roots).toHaveLength(60);

    // The session stays live — the handle is a snapshot, not a close.
    expect(documents.has(document_id)).toBe(true);
  });
});

describe("get_document body survives inline-UI slimming", () => {
  // Regression: get_document is in GEOMETRY_TOOLS, so on a client that
  // declared MCP Apps support, slimPreviewForInlineUi used to replace the
  // full IR body with a {document_id} stub whenever it exceeded 8192 chars —
  // which any real PCB does after place_components + route_nets. That broke
  // get_document's contract ("return the full IR Document JSON").

  /** A PCB session document whose serialized IR comfortably exceeds the
   *  8192-char slim threshold, mirroring a routed board. */
  function makeBigPcbDoc(): Document {
    const components = Array.from({ length: 120 }, (_, i) => ({
      ref: `R${i + 1}`,
      footprint: "0805",
      position: { x: i * 2.54, y: i * 1.27 },
      pins: [
        { number: "1", net: `NET_${i}` },
        { number: "2", net: "GND" },
      ],
    }));
    return {
      version: "0.1",
      nodes: {},
      materials: {},
      part_materials: {},
      roots: [{ root: 1, material: "default" }],
      pcb: {
        outline: {
          vertices: [
            [0, 0],
            [50, 0],
            [50, 40],
            [0, 40],
          ],
          thickness: 1.6,
        },
        components,
        nets: { GND: components.map((c) => `${c.ref}.2`) },
      },
    } as unknown as Document;
  }

  it("get_document returns the full IR (pcb + roots), not a document_id stub", () => {
    const text = JSON.stringify(makeBigPcbDoc());
    expect(text.length).toBeGreaterThan(8192);

    const result = { content: [{ type: "text", text }] };
    // clientHasInlineUi=true is the case that triggered the bug.
    slimPreviewForInlineUi(result, "doc_pcb", "get_document", true);

    const parsed = JSON.parse(result.content[0].text);
    expect(parsed.pcb).toBeDefined();
    expect(parsed.pcb.components).toHaveLength(120);
    expect(parsed.roots).toHaveLength(1);
    // The regression collapsed the body to exactly { document_id }.
    expect(Object.keys(parsed)).not.toEqual(["document_id"]);
  });

  it("still slims bulky mutation-tool results to a handle block", () => {
    const result = { content: [{ type: "text", text: "x".repeat(9000) }] };
    slimPreviewForInlineUi(result, "doc_pcb", "route_nets", true);
    // summary line + { document_id } block
    expect(result.content).toHaveLength(2);
    expect(JSON.parse(result.content[1].text)).toEqual({
      document_id: "doc_pcb",
    });
  });
});

describe("session persistence (save_document / load_document)", () => {
  let stateDir: string;
  let prevStateDir: string | undefined;

  beforeEach(() => {
    stateDir = mkdtempSync(join(tmpdir(), "vcad-mcp-state-"));
    prevStateDir = process.env.VCAD_MCP_STATE_DIR;
    process.env.VCAD_MCP_STATE_DIR = stateDir;
  });

  afterEach(() => {
    if (prevStateDir === undefined) delete process.env.VCAD_MCP_STATE_DIR;
    else process.env.VCAD_MCP_STATE_DIR = prevStateDir;
    rmSync(stateDir, { recursive: true, force: true });
  });

  it("round-trips a session to disk and back", async () => {
    const open = openDocument({ initial: makeCubeDoc() });
    const { document_id } = JSON.parse(open.content[0].text);

    const save = await saveDocument(
      { document_id, name: "my-part" },
      new InMemorySessionStore(),
    );
    const saved = JSON.parse(save.content[0].text);
    expect(saved.saved).toBe(true);
    expect(saved.name).toBe("my-part");
    expect(saved.path).toBe(join(stateDir, "my-part.vcad"));
    expect(existsSync(saved.path)).toBe(true);

    // Simulate a cold start: drop the in-process session.
    documents.clear();

    const load = await loadDocument(
      { name: "my-part" },
      new InMemorySessionStore(),
    );
    expect(load.isError).toBeFalsy();
    const loaded = JSON.parse(load.content[0].text);
    expect(loaded.document_id).toMatch(/^doc_/);
    expect(loaded.name).toBe("my-part");
    expect(loaded.parts).toBe(1);

    const fetched = JSON.parse(
      getDocumentTool({ document_id: loaded.document_id }).content[0].text,
    ) as Document;
    expect(fetched.roots).toHaveLength(1);
    expect(Object.keys(fetched.nodes)).toContain("1");
  });

  it("load_document on a missing file returns an isError result", async () => {
    const load = await loadDocument(
      { name: "does-not-exist" },
      new InMemorySessionStore(),
    );
    expect(load.isError).toBe(true);
    expect(load.content[0].text).toContain('No saved document named "does-not-exist"');
    expect(load.content[0].text).toContain(stateDir);
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
    // Real geometry satisfies the isoperimetric bound A³ ≥ 36πV², so the
    // impossibility warnings must be absent on a clean inspection.
    expect(props.warnings).toBeUndefined();
  });

  it("inspects an inline document with no resident session", () => {
    // Stateless escape hatch: no open_document, no document_id — pass the IR
    // directly (survives a cold serverless instance where the session is gone).
    documents.clear();
    const result = inspectCad({ document: makeCubeDoc() }, engine);
    const props = JSON.parse(result.content[0].text);
    expect(props.volume_mm3).toBeCloseTo(1000, 0);
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

  it("accepts an inline `document` alongside the legacy `ir` alias", () => {
    const filename = "test_export_document.stl";
    const filepath = resolve(process.cwd(), filename);
    if (existsSync(filepath)) unlinkSync(filepath);

    const result = exportCad({ document: makeCubeDoc(), filename }, engine);
    const output = JSON.parse(result.content[0].text);
    expect(output.format).toBe("stl");
    expect(output.bytes).toBeGreaterThan(84);
    expect(existsSync(filepath)).toBe(true);
    unlinkSync(filepath);
  });

  it("exports a resident session by document_id", () => {
    const filename = "test_export_session.stl";
    const filepath = resolve(process.cwd(), filename);
    if (existsSync(filepath)) unlinkSync(filepath);

    const { document_id } = JSON.parse(
      openDocument({ initial: makeCubeDoc() }).content[0].text,
    );
    const result = exportCad({ document_id, filename }, engine);
    const output = JSON.parse(result.content[0].text);
    expect(output.format).toBe("stl");
    expect(output.bytes).toBeGreaterThan(84);
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

  it("inspect_cad center of mass stays inside the bounding box", () => {
    // Regression: this flange chain tessellates to a non-watertight mesh
    // whose signed-volume integral partially cancels, which used to throw
    // the centroid below the bbox (z = -32.277 vs bbox min z = -31.439).
    const created = JSON.parse(
      sheetMetalCreate(
        {
          outline: [
            { x: 20, y: 0 },
            { x: 40, y: 0 },
            { x: 60, y: 40 },
            { x: 40, y: 80 },
            { x: 20, y: 80 },
            { x: 0, y: 40 },
          ],
          thickness: 0.5,
          material: "Al-soft",
          flanges: [
            { edge_index: 0, length: 35, angle: 1.1, direction: "Up" },
            {
              panel_id: 1,
              edge_index: 2,
              length: 12,
              angle: 2.2,
              direction: "Down",
            },
            { edge_index: 2, length: 50, angle: 0.6, direction: "Up" },
            { edge_index: 3, length: 30, angle: 0.9, direction: "Up" },
            { edge_index: 4, length: 50, angle: 0.6, direction: "Up" },
          ],
        },
        engine,
      ).content[0].text,
    );

    const result = JSON.parse(
      inspectCad({ document_id: created.document_id }, engine).content[0]
        .text,
    );
    const { bounding_box: bbox, center_of_mass: com } = result;
    for (const axis of ["x", "y", "z"] as const) {
      expect(com[axis]).toBeGreaterThanOrEqual(bbox.min[axis]);
      expect(com[axis]).toBeLessThanOrEqual(bbox.max[axis]);
    }
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


describe("mutation diff (changed)", () => {
  let document_id: string;

  beforeEach(() => {
    const open = openDocument({});
    document_id = JSON.parse(open.content[0].text).document_id;
  });

  it("create reports the new part under changed.added", () => {
    const out = dispatchRegistryTool("create", {
      document_id,
      type: "cube",
      name: "Base",
      params: { size: { x: 50, y: 30, z: 10 } },
    });
    const parsed = JSON.parse(out.content[0].text);
    expect(parsed.changed.added).toEqual([{ part_id: parsed.part_id, name: "Base" }]);
    expect(parsed.changed.removed).toEqual([]);
    expect(parsed.changed.modified).toEqual([]);
  });

  it("update reports the touched part under changed.modified; no-ops report nothing", () => {
    const created = JSON.parse(
      dispatchRegistryTool("create", {
        document_id,
        type: "cube",
        name: "Base",
        params: { size: { x: 50, y: 30, z: 10 } },
      }).content[0].text,
    );
    const updated = JSON.parse(
      dispatchRegistryTool("update", {
        document_id,
        node_id: created.node_id,
        params: { size: { x: 80, y: 30, z: 10 } },
      }).content[0].text,
    );
    expect(updated.changed.modified).toEqual([
      { part_id: created.part_id, name: "Base" },
    ]);

    const noop = JSON.parse(
      dispatchRegistryTool("update", {
        document_id,
        node_id: created.node_id,
        params: { size: { x: 80, y: 30, z: 10 } },
      }).content[0].text,
    );
    expect(noop.changed).toBeUndefined();
  });

  it("delete reports the part under changed.removed", () => {
    const created = JSON.parse(
      dispatchRegistryTool("create", {
        document_id,
        type: "cube",
        name: "Doomed",
        params: { size: { x: 5, y: 5, z: 5 } },
      }).content[0].text,
    );
    const out = JSON.parse(
      dispatchRegistryTool("delete", {
        document_id,
        part_id: created.part_id,
      }).content[0].text,
    );
    expect(out.changed.removed).toEqual([
      { part_id: created.part_id, name: "Doomed" },
    ]);
  });
});

describe("MCP description steering", () => {
  it("create steers whole-part work to create_cad_loon", () => {
    const create = registryToolDescriptors().find((t) => t.name === "create");
    expect(create?.description).toContain("create_cad_loon");
    const params = (create?.inputSchema.properties as Record<string, { description?: string }>).params;
    expect(params.description).not.toContain("system prompt");
  });

  it("set_material lists the preset keys inline", () => {
    const tool = registryToolDescriptors().find((t) => t.name === "set_material");
    expect(tool?.description).toContain("aluminum");
    expect(tool?.description).toContain("carbon-fiber");
  });
});

describe("tool packs (VCAD_MCP_PACKS)", () => {
  it("unset env disables nothing", async () => {
    const { disabledToolNames } = await import("../server.js");
    delete process.env.VCAD_MCP_PACKS;
    expect(disabledToolNames().size).toBe(0);
  });

  it("'none' disables every pack but leaves the core untouched", async () => {
    const { disabledToolNames } = await import("../server.js");
    process.env.VCAD_MCP_PACKS = "none";
    const disabled = disabledToolNames();
    delete process.env.VCAD_MCP_PACKS;
    expect(disabled.has("sheet_metal_create")).toBe(true);
    expect(disabled.has("run_drc")).toBe(true);
    expect(disabled.has("gym_step")).toBe(true);
    expect(disabled.has("verify_part")).toBe(true);
    expect(disabled.has("create")).toBe(false);
    expect(disabled.has("create_cad_loon")).toBe(false);
    expect(disabled.has("render_view")).toBe(false);
  });

  it("a named pack stays enabled while others drop", async () => {
    const { disabledToolNames } = await import("../server.js");
    process.env.VCAD_MCP_PACKS = "sheet_metal";
    const disabled = disabledToolNames();
    delete process.env.VCAD_MCP_PACKS;
    expect(disabled.has("sheet_metal_unfold")).toBe(false);
    expect(disabled.has("run_drc")).toBe(true);
  });
});

describe("GLB part-identity node names", () => {
  it("names glTF nodes <part_id>:<name> for the viewer's click-to-select", async () => {
    const { generateGlbPreview } = await import("../tools/preview.js");
    const engine = await Engine.init();
    const b64 = await generateGlbPreview(makeCubeDoc(), engine);
    expect(b64).toBeTruthy();
    const glb = Buffer.from(b64!, "base64");
    expect(glb.subarray(0, 4).toString()).toBe("glTF");
    const jsonLen = glb.readUInt32LE(12);
    const json = JSON.parse(glb.subarray(20, 20 + jsonLen).toString());
    expect(json.nodes[0].name).toBe("1:test_cube");
  });

  it("buildPartLabels skips hidden roots to stay aligned with scene.parts", async () => {
    const { buildPartLabels } = await import("../export/glb.js");
    const doc = makeCubeDoc();
    doc.nodes["2"] = { id: 2, name: "hidden_cube", op: { type: "Cube", size: { x: 5, y: 5, z: 5 } } };
    doc.roots.push({ root: 2, material: "default", visible: false });
    doc.nodes["3"] = { id: 3, name: null, op: { type: "Cube", size: { x: 2, y: 2, z: 2 } } };
    doc.roots.push({ root: 3, material: "default" });
    expect(buildPartLabels(doc)).toEqual(["1:test_cube", "3:"]);
  });
});

describe("create param defaults and error hints", () => {
  let document_id: string;

  beforeEach(() => {
    const open = openDocument({});
    document_id = JSON.parse(open.content[0].text).document_id;
  });

  it("fills segments for cylinder/sphere/cone so they aren't learned by failure", () => {
    for (const [type, params] of [
      ["cylinder", { radius: 5, height: 20 }],
      ["sphere", { radius: 5 }],
      ["cone", { radius_bottom: 5, radius_top: 0, height: 10 }],
    ] as const) {
      const result = dispatchRegistryTool("create", {
        document_id,
        type,
        params,
      });
      const parsed = JSON.parse(result.content[0].text);
      expect(parsed.part_id, `create ${type} without segments`).toBeTruthy();
    }
  });

  it("appends the expected param shape when the planner rejects create args", () => {
    expect(() =>
      dispatchRegistryTool("create", {
        document_id,
        type: "difference",
        params: { left: 1 }, // missing `right`
      }),
    ).toThrow(/Expected params for "difference".*\{left, right\}/s);
  });

  it("explains that booleans take node ids, not inline children", () => {
    expect(() =>
      dispatchRegistryTool("create", {
        document_id,
        type: "union",
        params: { left: { type: "cube" }, right: { type: "sphere" } },
      }),
    ).toThrow(/inline child definitions are not supported/);
  });
});
