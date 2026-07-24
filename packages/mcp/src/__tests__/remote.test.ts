import { describe, it, expect, afterEach, beforeAll } from "vitest";
import {
  isRemoteDeployment,
  maxInlineExportBytes,
  maxInlineArtifactBytes,
} from "../tools/remote.js";
import { importStep } from "../tools/import.js";
import { exportCad } from "../tools/export.js";
import { clearArtifacts } from "../tools/artifact-store.js";
import { getKernelWasm, type Engine } from "@vcad/engine";
import { createDocument } from "@vcad/ir";

afterEach(() => {
  delete process.env.VCAD_MCP_REMOTE;
  delete process.env.MCP_MAX_INLINE_EXPORT_BYTES;
  delete process.env.MCP_MAX_INLINE_ARTIFACT_BYTES;
  clearArtifacts();
});

describe("isRemoteDeployment / maxInlineExportBytes", () => {
  it("is off by default, on when VCAD_MCP_REMOTE=1", () => {
    expect(isRemoteDeployment()).toBe(false);
    process.env.VCAD_MCP_REMOTE = "1";
    expect(isRemoteDeployment()).toBe(true);
  });

  it("defaults the inline cap to 4 MiB and honors the override", () => {
    expect(maxInlineExportBytes()).toBe(4 * 1024 * 1024);
    process.env.MCP_MAX_INLINE_EXPORT_BYTES = "1024";
    expect(maxInlineExportBytes()).toBe(1024);
  });

  it("defaults the artifact cap to 64 KiB and honors the override", () => {
    expect(maxInlineArtifactBytes()).toBe(64 * 1024);
    process.env.MCP_MAX_INLINE_ARTIFACT_BYTES = "10";
    expect(maxInlineArtifactBytes()).toBe(10);
  });
});

describe("export_cad remote mode", () => {
  // Exercise the delivery branch through a tiny fake engine that returns a
  // single-part scene with a trivial mesh. The GLB/STL byte writers live in
  // kernel WASM, so the module still has to be initialized.
  beforeAll(async () => {
    await getKernelWasm();
  });

  const fakeEngine = {
    evaluate: () => ({
      parts: [
        {
          name: "p",
          mesh: {
            positions: new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]),
            indices: new Uint32Array([0, 1, 2]),
            normals: new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1]),
          },
          material: undefined,
        },
      ],
    }),
  } as unknown as Engine;

  it("returns inline base64 (no path) when remote and under the cap", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    const res = exportCad({ ir: createDocument(), filename: "cube.stl" }, fakeEngine);
    const out = JSON.parse(res.content[0].text);
    expect(out.data_base64).toBeTruthy();
    expect(out.path).toBeUndefined();
    expect(out.artifact_url).toBeUndefined();
    expect(out.filename).toBe("cube.stl");
    // base64 decodes to the reported byte count
    expect(Buffer.from(out.data_base64, "base64").length).toBe(out.bytes);
  });

  it("offloads to an artifact (URL + manifest) over the inline cap when remote", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    process.env.MCP_MAX_INLINE_EXPORT_BYTES = "10";
    const res = exportCad({ ir: createDocument(), filename: "cube.stl" }, fakeEngine);
    const out = JSON.parse(res.content[0].text);
    // No inline bytes — only the handle travels.
    expect(out.data_base64).toBeUndefined();
    expect(out.artifact_url).toMatch(/\/artifacts\/art_/);
    expect(out.artifact_id).toMatch(/^art_/);
    expect(Array.isArray(out.manifest)).toBe(true);
    expect(out.manifest).toHaveLength(1);
    expect(out.manifest[0].file).toBe("cube.stl");
    expect(out.manifest[0].bytes).toBe(out.bytes);
    expect(out.manifest[0].sha256).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe("import_step remote mode", () => {
  const fakeEngine = {} as Engine;

  it("rejects a filesystem path on a hosted server", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    expect(() => importStep({ filename: "part.step" }, fakeEngine)).toThrow(
      /content_base64/,
    );
  });

  it("requires filename or content_base64", () => {
    expect(() => importStep({}, fakeEngine)).toThrow(
      /filename.*content_base64|content_base64.*filename/,
    );
  });

  it("rejects empty base64 content", () => {
    expect(() => importStep({ content_base64: "" }, fakeEngine)).toThrow(
      /filename|content_base64/,
    );
  });
});
