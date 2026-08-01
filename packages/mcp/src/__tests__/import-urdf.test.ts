import { describe, it, expect, afterEach, beforeAll } from "vitest";
import { importUrdf } from "../tools/import-urdf.js";
import { getSession, documents } from "../tools/session.js";
import { Engine } from "@vcad/engine";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

afterEach(() => {
  delete process.env.VCAD_MCP_REMOTE;
  delete process.env.VCAD_MCP_EXPORT_DIR;
  documents.clear();
});

const EXAMPLES_DIR = resolve(__dirname, "../../../../examples");

function parseResult(res: { content: Array<{ type: string; text: string }> }) {
  return JSON.parse(res.content[0].text);
}

describe("import_urdf", () => {
  it("rejects filesystem paths in remote mode", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    expect(() => importUrdf({ path: "robot.urdf" }, engine)).toThrow(
      /no access to your filesystem/,
    );
  });

  it("requires path or content_base64", () => {
    expect(() => importUrdf({}, engine)).toThrow(
      /Provide either `path` or `content_base64`/,
    );
  });

  it("imports the 2-DOF arm fixture and registers a session", () => {
    process.env.VCAD_MCP_EXPORT_DIR = EXAMPLES_DIR;
    const out = parseResult(importUrdf({ path: "robot-arm-2dof.urdf" }, engine));
    expect(out.document_id).toMatch(/^doc_/);
    expect(out.summary.parts).toBeGreaterThan(0);
    expect(out.summary.joints).toBeGreaterThan(0);
    expect(out.summary.ground_instance_id).toBeTruthy();
    for (const j of out.summary.joint_list) {
      expect(j.id).toBeTruthy();
      expect(j.kind).toBeTruthy();
    }
    // The session is live — create_robot_env and render_view resolve it.
    expect(getSession(out.document_id)).toBeTruthy();
  });

  it("imports via content_base64 and reports unresolvable meshes", () => {
    const xml = `<robot name="r">
      <link name="base"><visual><geometry><mesh filename="meshes/base.STL"/></geometry></visual></link>
    </robot>`;
    const out = parseResult(
      importUrdf({ content_base64: Buffer.from(xml).toString("base64") }, engine),
    );
    expect(out.summary.unresolved_meshes).toHaveLength(1);
    expect(out.summary.warning).toMatch(/placeholder/);
  });

  it("inlines a binary STL referenced relative to the URDF", () => {
    const dir = mkdtempSync(join(tmpdir(), "urdf-test-"));
    try {
      // One-triangle binary STL: 80-byte header, count, 50 bytes.
      const stl = Buffer.alloc(84 + 50);
      stl.writeUInt32LE(1, 80);
      const tri = [0, 0, 1, /* normal */ 0, 0, 0, 1, 0, 0, 0, 1, 0];
      tri.forEach((v, i) => stl.writeFloatLE(v, 84 + i * 4));
      writeFileSync(join(dir, "tri.stl"), stl);
      writeFileSync(
        join(dir, "bot.urdf"),
        `<robot name="bot"><link name="base"><visual><geometry><mesh filename="tri.stl"/></geometry></visual></link></robot>`,
      );
      process.env.VCAD_MCP_EXPORT_DIR = dir;
      const out = parseResult(importUrdf({ path: "bot.urdf" }, engine));
      expect(out.summary.meshes_inlined).toBe(1);
      expect(out.summary.unresolved_meshes).toBeUndefined();
      const doc = getSession(out.document_id)!;
      const inlined = Object.values(doc.nodes).find(
        (n) => (n.op as { type: string }).type === "ImportedMesh",
      );
      expect(inlined).toBeTruthy();
      // Meters → mm: the 1m vertex coordinates come back as 1000.
      const op = inlined!.op as unknown as { positions: number[] };
      expect(Math.max(...op.positions)).toBe(1000);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("import_urdf floating base", () => {
  // Shaped like Booster's K1 and the Unitree descriptors: the world link
  // and its floating joint ship commented out, on the convention that the
  // simulator supplies the free base.
  const COMMENTED = `<robot name="k1">
      <!-- <link name="world"/>
      <joint name="world_joint" type="floating">
        <origin xyz="0 0 0"/>
        <parent link="world"/>
        <child link="Trunk"/>
      </joint> -->
      <link name="Trunk"/>
      <link name="Thigh"/>
      <joint name="hip" type="revolute">
        <parent link="Trunk"/><child link="Thigh"/>
        <axis xyz="0 1 0"/>
        <limit lower="-1" upper="1" effort="40" velocity="12.5"/>
      </joint>
    </robot>`;
  const b64 = () => Buffer.from(COMMENTED).toString("base64");

  it("warns when a floating joint is only present in a comment", () => {
    const out = parseResult(importUrdf({ content_base64: b64() }, engine));
    expect(out.summary.floating_base_warning).toMatch(/world_joint/);
    expect(out.summary.floating_base_warning).toMatch(/welded to the world/);
    // Behavior itself is unchanged: root still grounded, no Free joint.
    expect(out.summary.ground_instance_id).toBe("Trunk_inst");
    expect(out.summary.joint_list.some((j: { kind: string }) => j.kind === "Free")).toBe(false);
  });

  it("synthesizes a Free root joint with floating_base", () => {
    const out = parseResult(
      importUrdf(
        { content_base64: b64(), floating_base: true, spawn_height_mm: 620 },
        engine,
      ),
    );
    expect(out.summary.floating_base.synthesized).toBe(true);
    expect(out.summary.floating_base.spawn_height_mm).toBe(620);
    expect(out.summary.floating_base_warning).toBeUndefined();
    const free = out.summary.joint_list.filter(
      (j: { kind: string }) => j.kind === "Free",
    );
    expect(free).toHaveLength(1);
    expect(free[0].child_instance_id).toBe("Trunk_inst");
    // The world link is now the ground, not the Trunk.
    expect(out.summary.ground_instance_id).not.toBe("Trunk_inst");
  });

  it("rejects floating-base sub-options without floating_base", () => {
    expect(() =>
      importUrdf({ content_base64: b64(), spawn_height_mm: 620 }, engine),
    ).toThrow(/require floating_base/);
  });

  it("does not warn on a URDF with no floating joint at all", () => {
    const xml = `<robot name="arm">
        <link name="base"/><link name="tip"/>
        <joint name="j" type="revolute">
          <parent link="base"/><child link="tip"/><axis xyz="0 0 1"/>
        </joint>
      </robot>`;
    const out = parseResult(
      importUrdf({ content_base64: Buffer.from(xml).toString("base64") }, engine),
    );
    expect(out.summary.floating_base_warning).toBeUndefined();
  });
});
