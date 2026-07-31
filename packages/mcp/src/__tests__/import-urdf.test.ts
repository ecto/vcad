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
