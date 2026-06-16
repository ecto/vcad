import { describe, it, expect, afterEach } from "vitest";
import { isRemoteDeployment, maxInlineExportBytes } from "../tools/remote.js";
import { importStep } from "../tools/import.js";
import { exportCad } from "../tools/export.js";
import type { Engine } from "@vcad/engine";
import { createDocument } from "@vcad/ir";

afterEach(() => {
  delete process.env.VCAD_MCP_REMOTE;
  delete process.env.MCP_MAX_INLINE_EXPORT_BYTES;
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
});

describe("export_cad remote mode", () => {
  // A minimal one-cube document via the loon-free IR builder would require
  // the kernel; instead exercise the delivery branch through a tiny fake
  // engine that returns a single-part scene with a trivial mesh.
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

  it("returns inline base64 (no path) when remote", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    const res = exportCad({ ir: createDocument(), filename: "cube.stl" }, fakeEngine);
    const out = JSON.parse(res.content[0].text);
    expect(out.data_base64).toBeTruthy();
    expect(out.path).toBeUndefined();
    expect(out.filename).toBe("cube.stl");
    // base64 decodes to the reported byte count
    expect(Buffer.from(out.data_base64, "base64").length).toBe(out.bytes);
  });

  it("rejects exports over the inline cap when remote", () => {
    process.env.VCAD_MCP_REMOTE = "1";
    process.env.MCP_MAX_INLINE_EXPORT_BYTES = "10";
    expect(() =>
      exportCad({ ir: createDocument(), filename: "cube.stl" }, fakeEngine),
    ).toThrow(/inline limit/);
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
