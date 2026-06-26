import { describe, it, expect, afterEach, beforeEach } from "vitest";
import { Readable } from "node:stream";
import { createHash } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import {
  storeArtifact,
  getArtifact,
  getArtifactFile,
  resolveArtifact,
  resolveArtifactRef,
  parseArtifactId,
  buildManifest,
  bundleBytes,
  clearArtifacts,
} from "../tools/artifact-store.js";
import { handleArtifactRequest } from "../artifact-route.js";
import { importStep } from "../tools/import.js";
import { documents } from "../tools/session.js";
import type { Engine } from "@vcad/engine";

const sha = (s: string) => createHash("sha256").update(Buffer.from(s, "utf8")).digest("hex");

afterEach(() => {
  clearArtifacts();
  delete process.env.MCP_MAX_INLINE_ARTIFACT_BYTES;
  delete process.env.VCAD_MCP_PUBLIC_URL;
  documents.clear();
});

describe("artifact-store", () => {
  it("hashes a manifest and counts bundle bytes without storing", () => {
    const files = [
      { name: "a.gbr", content: "AAAA" },
      { name: "b.drl", content: "BB" },
    ];
    expect(bundleBytes(files)).toBe(6);
    const manifest = buildManifest(files);
    expect(manifest).toEqual([
      { file: "a.gbr", bytes: 4, sha256: sha("AAAA") },
      { file: "b.drl", bytes: 2, sha256: sha("BB") },
    ]);
  });

  it("stores a bundle and round-trips file bytes by id", () => {
    const handle = storeArtifact([
      { name: "top.gbr", content: "G04 top*" },
      { name: "out.drl", content: "M48" },
    ]);
    expect(handle.artifact_id).toMatch(/^art_/);
    expect(handle.artifact_url).toMatch(/\/artifacts\/art_/);
    expect(handle.bytes).toBe("G04 top*".length + "M48".length);
    expect(handle.manifest).toHaveLength(2);

    const stored = getArtifact(handle.artifact_id);
    expect(stored).not.toBeNull();
    expect(stored!.files).toHaveLength(2);

    const file = getArtifactFile(handle.artifact_id, "top.gbr");
    expect(file).not.toBeNull();
    expect(file!.buf.toString("utf8")).toBe("G04 top*");
  });

  it("resolves a handle by raw id and by artifact_url", () => {
    process.env.VCAD_MCP_PUBLIC_URL = "https://mcp.example.com";
    const handle = storeArtifact([{ name: "x.gbr", content: "hi" }]);

    expect(parseArtifactId(handle.artifact_id)).toBe(handle.artifact_id);
    expect(parseArtifactId(handle.artifact_url)).toBe(handle.artifact_id);

    expect(resolveArtifact(handle.artifact_id)?.id).toBe(handle.artifact_id);
    expect(resolveArtifact(handle.artifact_url)?.id).toBe(handle.artifact_id);

    const ref = resolveArtifactRef(handle.artifact_url);
    expect(ref?.artifact_id).toBe(handle.artifact_id);
    expect(ref?.bytes).toBe(2);
    expect(ref?.manifest).toHaveLength(1);

    expect(resolveArtifact("art_nope")).toBeNull();
    expect(resolveArtifactRef("art_nope")).toBeNull();
  });
});

// ── HTTP route ────────────────────────────────────────────────────────────

function makeReq(method: string, path: string): IncomingMessage {
  const req = Readable.from([]);
  Object.assign(req, { method, url: path, headers: { host: "mcp.vcad.io" } });
  return req as unknown as IncomingMessage;
}
interface CapRes {
  statusCode: number;
  headers: Record<string, string>;
  body: string;
  bodyBuf: Buffer | null;
  writeHead(s: number, h?: Record<string, string>): CapRes;
  end(b?: string | Buffer): void;
}
function makeRes(): CapRes {
  return {
    statusCode: 0,
    headers: {},
    body: "",
    bodyBuf: null,
    writeHead(s, h) {
      this.statusCode = s;
      if (h) this.headers = h;
      return this;
    },
    end(b) {
      if (Buffer.isBuffer(b)) {
        this.bodyBuf = b;
        this.body = b.toString("utf8");
      } else {
        this.body = b ?? "";
      }
    },
  };
}
const res = () => makeRes() as unknown as ServerResponse & CapRes;

describe("handleArtifactRequest", () => {
  it("ignores non-/artifacts paths", async () => {
    const r = res();
    expect(await handleArtifactRequest(makeReq("GET", "/mcp"), r)).toBe(false);
  });

  it("serves the manifest index and each file's raw bytes", async () => {
    const handle = storeArtifact([
      { name: "top.gbr", content: "G04 top*" },
      { name: "out.drl", content: "M48" },
    ]);

    const idx = res();
    expect(await handleArtifactRequest(makeReq("GET", `/artifacts/${handle.artifact_id}`), idx)).toBe(true);
    expect(idx.statusCode).toBe(200);
    const manifest = JSON.parse(idx.body);
    expect(manifest.artifact_id).toBe(handle.artifact_id);
    expect(manifest.files).toHaveLength(2);
    expect(manifest.files[0].url).toMatch(/\/artifacts\/art_.+\/top\.gbr$/);

    const file = res();
    expect(
      await handleArtifactRequest(makeReq("GET", `/artifacts/${handle.artifact_id}/top.gbr`), file),
    ).toBe(true);
    expect(file.statusCode).toBe(200);
    expect(file.headers["Content-Type"]).toBe("application/vnd.gerber");
    expect(file.bodyBuf?.toString("utf8")).toBe("G04 top*");
  });

  it("404s an unknown artifact id", async () => {
    const r = res();
    expect(await handleArtifactRequest(makeReq("GET", "/artifacts/art_missing"), r)).toBe(true);
    expect(r.statusCode).toBe(404);
  });
});

// ── import_step offload ─────────────────────────────────────────────────────

describe("import_step large-result offload", () => {
  const smallEngine = {
    importStep: () => [
      {
        positions: new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]),
        indices: new Uint32Array([0, 1, 2]),
        normals: undefined,
      },
    ],
  } as unknown as Engine;

  const bigEngine = {
    importStep: () => [
      {
        positions: new Float32Array(40_000),
        indices: new Uint32Array(40_000),
        normals: undefined,
      },
    ],
  } as unknown as Engine;

  const step64 = Buffer.from("x").toString("base64");

  it("keeps a small import inline", () => {
    const res = importStep({ content_base64: step64, name: "tiny" }, smallEngine);
    const out = JSON.parse(res.content[0].text);
    expect(out.document).toBeTruthy();
    expect(out.document.version).toBeTruthy();
    expect(out.artifact_url).toBeUndefined();
  });

  it("offloads a large import to a session + artifact handle (no inline IR)", () => {
    const res = importStep({ content_base64: step64, name: "huge" }, bigEngine);
    const out = JSON.parse(res.content[0].text);
    // The multi-MB IR never enters context.
    expect(out.document).toBeUndefined();
    expect(out.document_id).toBeTruthy();
    expect(documents.has(out.document_id)).toBe(true);
    expect(out.artifact_url).toMatch(/\/artifacts\/art_/);
    expect(out.manifest).toHaveLength(1);
    expect(out.manifest[0].file).toBe("huge.vcad");
    expect(out.summary.bodies).toBe(1);
  });
});
