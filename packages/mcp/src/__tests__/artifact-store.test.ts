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

  it("mints relative URLs when memory-only, absolute mcp.vcad.io when durable", () => {
    // Memory-only (no VCAD_MCP_PUBLIC_URL, no Supabase env): an absolute
    // mcp.vcad.io link would be a guaranteed 404 — that host never saw the
    // bytes. The handle stays relative, which trust-boundary accepts.
    delete process.env.SUPABASE_URL;
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    const local = storeArtifact([{ name: "x.gbr", content: "hi" }]);
    expect(local.artifact_url).toBe(`/artifacts/${local.artifact_id}`);
    expect(parseArtifactId(local.artifact_url)).toBe(local.artifact_id);

    // Durable store: the persisted row IS readable from the hosted origin.
    process.env.SUPABASE_URL = "https://fake.supabase.co";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "service-role-test-key";
    try {
      const hosted = storeArtifact([{ name: "y.gbr", content: "hi" }]);
      expect(hosted.artifact_url).toBe(
        `https://mcp.vcad.io/artifacts/${hosted.artifact_id}`,
      );
    } finally {
      delete process.env.SUPABASE_URL;
      delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    }
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
    importStepWithReport: () => ({
      meshes: [
        {
          positions: new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]),
          indices: new Uint32Array([0, 1, 2]),
          normals: undefined,
        },
      ],
      report: [],
      summary: null,
    }),
  } as unknown as Engine;

  const bigEngine = {
    importStepWithReport: () => ({
      meshes: [
        {
          positions: new Float32Array(40_000),
          indices: new Uint32Array(40_000),
          normals: undefined,
        },
      ],
      report: [],
      summary: null,
    }),
  } as unknown as Engine;

  const step64 = Buffer.from("x").toString("base64");

  // These cases are about report surfacing and large-result offload, not about
  // which representation import_step defaults to — the stub engines implement
  // only the mesh entry point, so they ask for `as_mesh` explicitly. B-rep
  // import is covered in step-import.test.ts against the real kernel.


  it("surfaces skipped faces from the import report", () => {
    const dirtyEngine = {
      importStepWithReport: () => ({
        meshes: [
          {
            positions: new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]),
            indices: new Uint32Array([0, 1, 2]),
            normals: undefined,
          },
        ],
        report: [
          {
            solid_id: 45,
            total_faces: 3,
            skipped_faces: [
              { face_id: 29, surface_id: 40, reason: "DEGENERATE_TOROIDAL_SURFACE" },
            ],
            notes: [],
          },
        ],
        summary: "solid #45: skipped 1 of 3 faces",
      }),
    } as unknown as Engine;
    const res = importStep({ content_base64: step64, name: "holey", as_mesh: true }, dirtyEngine);
    const out = JSON.parse(res.content[0].text);
    expect(out.summary.warning).toContain("1 face(s) skipped");
    expect(out.summary.skipped_faces).toEqual([
      { solid_id: 45, face_id: 29, surface_id: 40, reason: "DEGENERATE_TOROIDAL_SURFACE" },
    ]);
  });

  it("keeps a small import inline", () => {
    const res = importStep({ content_base64: step64, name: "tiny", as_mesh: true }, smallEngine);
    const out = JSON.parse(res.content[0].text);
    expect(out.document).toBeTruthy();
    expect(out.document.version).toBeTruthy();
    expect(out.artifact_url).toBeUndefined();
  });

  it("offloads a large import to a session + artifact handle (no inline IR)", () => {
    const res = importStep({ content_base64: step64, name: "huge", as_mesh: true }, bigEngine);
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

// ── Durable backend (mcp_artifacts) ─────────────────────────────────────────
//
// Fakes the PostgREST surface via the injectable sessionFetch seam (the same
// hook the session-store tests use) and simulates a serverless COLD START by
// clearing the warm registry between the write and the read — the exact
// failure that shipped: a handle minted on one instance was unreadable on
// every other ("Unknown or expired" / a 404 artifact_url).

import {
  getArtifactAsync,
  getArtifactFileAsync,
  resolveArtifactRefAsync,
  flushArtifacts,
  artifactStoreInfo,
} from "../tools/artifact-store.js";
import { setSessionFetch } from "../session-store.js";

interface FakeRow {
  artifact_id: string;
  bytes: number;
  manifest: unknown;
  files: unknown;
  expires_at: string;
}

function fakeSupabase(rows: Map<string, FakeRow>): {
  fetch: typeof fetch;
  calls: string[];
} {
  const calls: string[] = [];
  const reply = (status: number, body: unknown = {}) =>
    ({
      ok: status >= 200 && status < 300,
      status,
      json: async () => body,
      text: async () => JSON.stringify(body),
    }) as unknown as Response;

  const fetchImpl = (async (input: unknown, init?: RequestInit) => {
    const url = new URL(String(input));
    const method = init?.method ?? "GET";
    calls.push(`${method} ${url.pathname}${url.search}`);
    if (!url.pathname.endsWith("/rest/v1/mcp_artifacts")) return reply(404);
    const idFilter = url.searchParams.get("artifact_id"); // "eq.<id>"
    const id = idFilter?.startsWith("eq.") ? idFilter.slice(3) : null;
    if (method === "POST") {
      const body = JSON.parse(String(init?.body ?? "[]")) as FakeRow[];
      for (const row of body) rows.set(row.artifact_id, row);
      return reply(201);
    }
    if (method === "GET") {
      const row = id ? rows.get(id) : undefined;
      return row ? reply(200, row) : reply(406, {});
    }
    if (method === "DELETE") {
      if (id) rows.delete(id);
      return reply(204);
    }
    return reply(405);
  }) as typeof fetch;

  return { fetch: fetchImpl, calls };
}

describe("durable artifact store (mcp_artifacts)", () => {
  const rows = new Map<string, FakeRow>();
  let calls: string[] = [];

  beforeEach(() => {
    rows.clear();
    process.env.SUPABASE_URL = "https://fake.supabase.co";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "service-role-test-key";
    const fake = fakeSupabase(rows);
    calls = fake.calls;
    setSessionFetch(fake.fetch);
  });

  afterEach(() => {
    delete process.env.SUPABASE_URL;
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    setSessionFetch((...args) => fetch(...args));
  });

  it("reports durability from the shared Supabase env", () => {
    expect(artifactStoreInfo()).toEqual({ artifact_store: "supabase" });
    delete process.env.SUPABASE_URL;
    expect(artifactStoreInfo()).toEqual({ artifact_store: "in-memory" });
  });

  it("persists on store and hydrates on a cold instance (the shipped bug)", async () => {
    const handle = storeArtifact([
      { name: "F_Cu.gbr", content: "G04 fcu*" },
      { name: "drill.drl", content: "M48" },
    ]);
    await flushArtifacts();
    expect(rows.has(handle.artifact_id)).toBe(true);

    // Cold start: the warm registry is gone; only the durable row survives.
    clearArtifacts();
    expect(getArtifact(handle.artifact_id)).toBeNull(); // sync path misses

    const a = await getArtifactAsync(handle.artifact_id);
    expect(a).not.toBeNull();
    expect(a!.manifest).toEqual(handle.manifest);
    const f = await getArtifactFileAsync(handle.artifact_id, "F_Cu.gbr");
    expect(f!.buf.toString("utf8")).toBe("G04 fcu*");
    expect(f!.contentType).toBe("application/vnd.gerber");

    // The hydrate warmed the cache — the next read is sync, no extra fetch.
    const fetches = calls.length;
    expect(getArtifact(handle.artifact_id)).not.toBeNull();
    expect(calls.length).toBe(fetches);
  });

  it("binds a cross-instance handle for quote/order (resolveArtifactRefAsync)", async () => {
    const handle = storeArtifact([{ name: "board.zip", content: "PK" }]);
    await flushArtifacts();
    clearArtifacts();

    const ref = await resolveArtifactRefAsync(handle.artifact_url);
    expect(ref).not.toBeNull();
    expect(ref!.artifact_id).toBe(handle.artifact_id);
    expect(ref!.manifest).toEqual(handle.manifest);
    expect(await resolveArtifactRefAsync("art_nope")).toBeNull();
  });

  it("serves a cold-instance read over the /artifacts route", async () => {
    const handle = storeArtifact([{ name: "Edge_Cuts.gbr", content: "G04 edge*" }]);
    await flushArtifacts();
    clearArtifacts();

    const idx = res();
    expect(
      await handleArtifactRequest(makeReq("GET", `/artifacts/${handle.artifact_id}`), idx),
    ).toBe(true);
    expect(idx.statusCode).toBe(200);
    expect(JSON.parse(idx.body).files).toHaveLength(1);

    const file = res();
    expect(
      await handleArtifactRequest(
        makeReq("GET", `/artifacts/${handle.artifact_id}/Edge_Cuts.gbr`),
        file,
      ),
    ).toBe(true);
    expect(file.statusCode).toBe(200);
    expect(file.bodyBuf?.toString("utf8")).toBe("G04 edge*");
  });

  it("treats an expired durable row as absent and deletes it", async () => {
    const handle = storeArtifact([{ name: "x.gbr", content: "hi" }]);
    await flushArtifacts();
    clearArtifacts();
    const row = rows.get(handle.artifact_id)!;
    row.expires_at = new Date(Date.now() - 1000).toISOString();

    expect(await getArtifactAsync(handle.artifact_id)).toBeNull();
    // Lazy sweep: the expired row was deleted (fire-and-forget).
    await new Promise((r) => setTimeout(r, 0));
    expect(rows.has(handle.artifact_id)).toBe(false);
  });

  it("degrades to warm-cache-only when the durable write fails", async () => {
    setSessionFetch((async () =>
      ({ ok: false, status: 500, json: async () => ({}), text: async () => "boom" })
    ) as unknown as typeof fetch);
    const handle = storeArtifact([{ name: "y.gbr", content: "yo" }]);
    await flushArtifacts();
    // Same-instance read still works off the warm cache.
    expect((await getArtifactAsync(handle.artifact_id))?.id).toBe(handle.artifact_id);
  });
});
