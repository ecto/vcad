/**
 * Artifact store — keep large tool outputs OUT of model context.
 *
 * Export tools (export_gerber, export_cad, import_step) can produce payloads
 * far larger than an MCP tool result should carry: a Gerber fab bundle is
 * ~11–13 files / ~168 KB, which blows the model's tool-output token budget if
 * returned inline and is unusable through the channel. The byte-cap idiom
 * (see maxInlineArtifactBytes / maxInlineExportBytes in remote.ts) decides per
 * call — small payloads stay inline; large ones are written HERE and the tool
 * returns a compact { artifact_url, manifest } handle instead. The bytes then
 * ride the artifact channel (the /artifacts/<id> HTTP route), never the model's
 * context. quote_manufacturing / place_order accept the same handle, so an
 * order references the bundle without ever re-sending the files.
 *
 * Backing: an in-process, bounded, TTL'd registry (a warm-instance CACHE)
 * in front of a durable Supabase table (`mcp_artifacts`, migration 033) —
 * the same hydrate-on-miss / persist-after-write model as session-store.ts,
 * and gated by the same env (SUPABASE_URL + SUPABASE_SERVICE_ROLE_KEY).
 * Without that env (stdio/local) the in-memory registry alone reproduces the
 * old behavior. WHY: the hosted MCP runs as a serverless function, so a
 * handle minted on one instance was unreadable on every other — the
 * /artifacts URL 404'd from a cold instance and quote_manufacturing rejected
 * a fab_artifact_id minted minutes earlier by export_gerber ("Unknown or
 * expired"). Writes stay SYNC for callers: storeArtifact caches, kicks off a
 * best-effort durable persist, and the entrypoint awaits flushArtifacts()
 * before the instance can freeze (the flushTelemetry idiom). Reads that must
 * work cross-instance go through the async getters, which fall back to the
 * durable row on a warm-cache miss.
 */

import { createHash, randomBytes } from "node:crypto";
import { sessionFetch } from "../session-store.js";

/** One file handed to the store (Gerber text, an STL/GLB binary, an IR doc). */
export interface ArtifactInputFile {
  name: string;
  content: string | Uint8Array;
  /** Optional MIME type; inferred from the extension when omitted. */
  contentType?: string;
}

/** One manifest row: the file, its size, and a content hash for verification. */
export interface ManifestEntry {
  file: string;
  bytes: number;
  sha256: string;
}

/** The compact handle a tool returns in place of inline bytes. */
export interface ArtifactHandle {
  artifact_id: string;
  /** Index URL — GETs the manifest; `${artifact_url}/<file>` GETs one file. */
  artifact_url: string;
  manifest: ManifestEntry[];
  /** Total bundle size across all files. */
  bytes: number;
  expires_at: string;
}

interface StoredFile {
  name: string;
  buf: Buffer;
  contentType: string;
}

interface StoredArtifact {
  id: string;
  files: StoredFile[];
  manifest: ManifestEntry[];
  bytes: number;
  createdAt: number;
  expiresAt: number;
}

// Module-global registry — survives across tool calls on one warm instance
// (same lifetime model as the in-memory session/order maps).
const registry = new Map<string, StoredArtifact>();

function ttlMs(): number {
  const raw = process.env.MCP_ARTIFACT_TTL_MS;
  const n = raw ? parseInt(raw, 10) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 24 * 60 * 60 * 1000; // 24h
}

function maxEntries(): number {
  const raw = process.env.MCP_ARTIFACT_MAX_ENTRIES;
  const n = raw ? parseInt(raw, 10) : NaN;
  return Number.isFinite(n) && n > 0 ? n : 256;
}

/** Drop expired entries, then bound the registry to maxEntries (oldest-first;
 *  Map preserves insertion order). Keeps a long-running server from growing
 *  without limit. */
function evict(now: number): void {
  for (const [id, a] of registry) {
    if (a.expiresAt <= now) registry.delete(id);
  }
  while (registry.size >= maxEntries()) {
    const oldest = registry.keys().next().value;
    if (oldest === undefined) break;
    registry.delete(oldest);
  }
}

/** Public base URL of this deployment, for building shareable artifact links.
 *  Mirrors the live-share link builder. Empty string → links stay relative
 *  (`/artifacts/<id>`), which trust-boundary and parseArtifactId both accept. */
function publicBaseUrl(): string {
  const raw = process.env.VCAD_MCP_PUBLIC_URL;
  if (raw) {
    try {
      return new URL(raw).origin;
    } catch {
      // fall through to the durability-gated default
    }
  }
  // Only a durable store may claim the hosted origin by default: the persisted
  // row is readable from mcp.vcad.io. A memory-only instance (stdio/local dev)
  // minting an absolute mcp.vcad.io link hands out a guaranteed 404 — its
  // bytes exist nowhere that host can see.
  return isArtifactStoreDurable() ? "https://mcp.vcad.io" : "";
}

/** The capability URL for an artifact id (possession of the id is the grant). */
export function artifactUrl(id: string): string {
  return `${publicBaseUrl()}/artifacts/${encodeURIComponent(id)}`;
}

/** The URL for a single file within an artifact. */
export function artifactFileUrl(id: string, file: string): string {
  return `${artifactUrl(id)}/${encodeURIComponent(file)}`;
}

function toBuffer(content: string | Uint8Array): Buffer {
  return typeof content === "string"
    ? Buffer.from(content, "utf8")
    : Buffer.from(content);
}

function guessContentType(name: string): string {
  const ext = name.toLowerCase().split(".").pop() ?? "";
  switch (ext) {
    case "stl":
      return "model/stl";
    case "glb":
      return "model/gltf-binary";
    case "step":
    case "stp":
      return "application/step";
    case "json":
    case "vcad":
      return "application/json";
    case "csv":
      return "text/csv";
    case "gbr":
    case "gbl":
    case "gtl":
    case "gto":
    case "gbs":
    case "gts":
    case "gko":
      return "application/vnd.gerber";
    case "drl":
    case "txt":
      return "text/plain";
    case "zip":
      return "application/zip";
    case "png":
      return "image/png";
    case "svg":
      return "image/svg+xml";
    case "gif":
      return "image/gif";
    default:
      return "application/octet-stream";
  }
}

/** Total byte count of a bundle — the value compared against the inline cap. */
export function bundleBytes(files: ArtifactInputFile[]): number {
  return files.reduce((n, f) => n + toBuffer(f.content).length, 0);
}

/** Build a manifest (file, bytes, sha256) without storing anything. */
export function buildManifest(files: ArtifactInputFile[]): ManifestEntry[] {
  return files.map((f) => {
    const buf = toBuffer(f.content);
    return {
      file: f.name,
      bytes: buf.length,
      sha256: createHash("sha256").update(buf).digest("hex"),
    };
  });
}

function newArtifactId(): string {
  // Unguessable — the id IS the capability to read the bundle (same model as
  // live-share session ids).
  return `art_${randomBytes(12).toString("base64url")}`;
}

/** Store a bundle and return its handle (id, url, manifest). Sync for
 *  callers; the durable persist runs in the background and is drained by
 *  flushArtifacts() before a serverless instance can freeze. */
export function storeArtifact(files: ArtifactInputFile[]): ArtifactHandle {
  const now = Date.now();
  evict(now);
  const id = newArtifactId();
  const stored: StoredFile[] = files.map((f) => ({
    name: f.name,
    buf: toBuffer(f.content),
    contentType: f.contentType ?? guessContentType(f.name),
  }));
  const manifest = stored.map((f) => ({
    file: f.name,
    bytes: f.buf.length,
    sha256: createHash("sha256").update(f.buf).digest("hex"),
  }));
  const bytes = stored.reduce((n, f) => n + f.buf.length, 0);
  const expiresAt = now + ttlMs();
  const artifact: StoredArtifact = {
    id,
    files: stored,
    manifest,
    bytes,
    createdAt: now,
    expiresAt,
  };
  registry.set(id, artifact);
  trackPersist(persistArtifact(artifact));
  return {
    artifact_id: id,
    artifact_url: artifactUrl(id),
    manifest,
    bytes,
    expires_at: new Date(expiresAt).toISOString(),
  };
}

/** Read a stored artifact by id from the WARM CACHE only (expired → absent).
 *  Cross-instance callers want getArtifactAsync, which falls back to the
 *  durable row on a miss. */
export function getArtifact(id: string): StoredArtifact | null {
  const a = registry.get(id);
  if (!a) return null;
  if (a.expiresAt <= Date.now()) {
    registry.delete(id);
    return null;
  }
  return a;
}

/** Read one file from a stored artifact (warm cache only — see getArtifact). */
export function getArtifactFile(id: string, name: string): StoredFile | null {
  const a = getArtifact(id);
  if (!a) return null;
  return a.files.find((f) => f.name === name) ?? null;
}

// ─── Durable backend (mcp_artifacts, migration 033) ──────────────────────────
//
// Same raw-PostgREST/service-role pattern (and the same injectable
// `sessionFetch` seam) as session-store.ts. Capability-keyed by the
// unguessable artifact id alone — RLS is on with no anon/authenticated
// policies, so only the server ever reads or writes rows. Best-effort
// throughout: a durable outage degrades to warm-cache-only (the old
// behavior), never a tool failure.

interface ArtifactRow {
  artifact_id: string;
  bytes: number;
  manifest: ManifestEntry[];
  /** Files with base64 content — jsonb can't hold raw bytes. */
  files: Array<{ name: string; content_type: string; b64: string }>;
  created_at?: string;
  expires_at: string;
}

function durableEnv(): { url: string; key: string } | null {
  const url = (process.env.SUPABASE_URL || "").replace(/\/+$/, "");
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || "";
  return url && key ? { url, key } : null;
}

/** True when artifact handles survive across serverless instances. Same env
 *  gate as isSessionStoreDurable — the two stores share the backend. */
export function isArtifactStoreDurable(): boolean {
  return durableEnv() !== null;
}

/** Durability state for server_info / the /health endpoint. */
export function artifactStoreInfo(): {
  artifact_store: "supabase" | "in-memory";
} {
  return { artifact_store: isArtifactStoreDurable() ? "supabase" : "in-memory" };
}

function durableHeaders(key: string, extra: Record<string, string> = {}): Record<string, string> {
  return {
    apikey: key,
    Authorization: `Bearer ${key}`,
    "Content-Type": "application/json",
    ...extra,
  };
}

/** In-flight durable persists, drained by flushArtifacts() — the
 *  flushTelemetry idiom: a serverless instance freezes the moment the
 *  response is written, killing any un-awaited fetch. */
const pendingPersists = new Set<Promise<void>>();

function trackPersist(p: Promise<void>): void {
  pendingPersists.add(p);
  void p.finally(() => pendingPersists.delete(p));
}

/** Await outstanding durable writes (bounded). The HTTP entrypoints call this
 *  before returning, alongside flushTelemetry. */
export async function flushArtifacts(timeoutMs = 5000): Promise<void> {
  if (pendingPersists.size === 0) return;
  const timeout = new Promise<void>((resolve) => {
    const t = setTimeout(resolve, timeoutMs);
    // Don't hold the event loop open just for the flush timer.
    if (typeof t === "object" && "unref" in t) t.unref();
  });
  await Promise.race([Promise.allSettled([...pendingPersists]), timeout]);
}

/** Write-through to the durable table. Best-effort: logs, never throws. */
async function persistArtifact(a: StoredArtifact): Promise<void> {
  const env = durableEnv();
  if (!env) return;
  try {
    const row: ArtifactRow = {
      artifact_id: a.id,
      bytes: a.bytes,
      manifest: a.manifest,
      files: a.files.map((f) => ({
        name: f.name,
        content_type: f.contentType,
        b64: f.buf.toString("base64"),
      })),
      expires_at: new Date(a.expiresAt).toISOString(),
    };
    const res = await sessionFetch(
      `${env.url}/rest/v1/mcp_artifacts?on_conflict=artifact_id`,
      {
        method: "POST",
        headers: durableHeaders(env.key, {
          Prefer: "resolution=merge-duplicates,return=minimal",
        }),
        body: JSON.stringify([row]),
      },
    );
    if (!res.ok) {
      console.error(
        "[artifact-store] persist failed:",
        res.status,
        await res.text().catch(() => ""),
      );
    }
  } catch (err) {
    console.error("[artifact-store] persist failed:", err);
  }
}

/** Best-effort delete of an expired durable row. */
async function dropDurable(id: string): Promise<void> {
  const env = durableEnv();
  if (!env) return;
  try {
    await sessionFetch(
      `${env.url}/rest/v1/mcp_artifacts?artifact_id=eq.${encodeURIComponent(id)}`,
      { method: "DELETE", headers: durableHeaders(env.key) },
    );
  } catch (err) {
    console.error("[artifact-store] expired-row delete failed:", err);
  }
}

/** Fetch a durable row and rehydrate it into the warm cache. Null on miss,
 *  expiry (the row is then deleted), malformed content, or any error. */
async function loadDurable(id: string): Promise<StoredArtifact | null> {
  const env = durableEnv();
  if (!env) return null;
  try {
    const res = await sessionFetch(
      `${env.url}/rest/v1/mcp_artifacts` +
        `?artifact_id=eq.${encodeURIComponent(id)}` +
        `&select=artifact_id,bytes,manifest,files,created_at,expires_at&limit=1`,
      {
        method: "GET",
        headers: durableHeaders(env.key, {
          Accept: "application/vnd.pgrst.object+json",
        }),
      },
    );
    if (!res.ok) return null; // 406 = zero rows → miss
    const row = (await res.json()) as ArtifactRow;
    if (!row || row.artifact_id !== id || !Array.isArray(row.files)) return null;
    const expiresAt = Date.parse(row.expires_at);
    if (!Number.isFinite(expiresAt)) return null;
    if (expiresAt <= Date.now()) {
      void dropDurable(id);
      return null;
    }
    const files: StoredFile[] = row.files.map((f) => ({
      name: String(f.name),
      buf: Buffer.from(String(f.b64 ?? ""), "base64"),
      contentType: String(f.content_type || guessContentType(String(f.name))),
    }));
    const artifact: StoredArtifact = {
      id,
      files,
      manifest: Array.isArray(row.manifest)
        ? row.manifest
        : files.map((f) => ({
            file: f.name,
            bytes: f.buf.length,
            sha256: createHash("sha256").update(f.buf).digest("hex"),
          })),
      bytes:
        typeof row.bytes === "number"
          ? row.bytes
          : files.reduce((n, f) => n + f.buf.length, 0),
      createdAt: row.created_at ? Date.parse(row.created_at) : Date.now(),
      expiresAt,
    };
    // Rehydrate the warm cache so subsequent same-instance reads are sync.
    evict(Date.now());
    registry.set(id, artifact);
    return artifact;
  } catch (err) {
    console.error("[artifact-store] load failed:", err);
    return null;
  }
}

/** Read an artifact by id: warm cache first, then the durable row. The
 *  cross-instance read path — the /artifacts route and the order tools use
 *  this, so a handle minted on another instance still resolves. */
export async function getArtifactAsync(id: string): Promise<StoredArtifact | null> {
  return getArtifact(id) ?? (await loadDurable(id));
}

/** Read one file from an artifact (warm cache, then durable). */
export async function getArtifactFileAsync(
  id: string,
  name: string,
): Promise<StoredFile | null> {
  const a = await getArtifactAsync(id);
  if (!a) return null;
  return a.files.find((f) => f.name === name) ?? null;
}

/**
 * Resolve a caller-supplied artifact handle — accepts a raw id or a full
 * artifact_url (…/artifacts/<id>[/<file>]) — to the stored bundle, or null if
 * unknown/expired. Used by quote_manufacturing / place_order so the fab files
 * are fetched from the store, never re-sent through model context.
 */
export function resolveArtifact(handle: string): StoredArtifact | null {
  const id = parseArtifactId(handle);
  return id ? getArtifact(id) : null;
}

/** resolveArtifact with the durable fallback — the cross-instance path. */
export async function resolveArtifactAsync(
  handle: string,
): Promise<StoredArtifact | null> {
  const id = parseArtifactId(handle);
  return id ? getArtifactAsync(id) : null;
}

/** A persistable reference to a stored bundle — metadata only, never bytes. */
export interface ArtifactRef {
  artifact_id: string;
  artifact_url: string;
  bytes: number;
  manifest: ManifestEntry[];
}

/** Resolve a handle to a metadata-only ref suitable for attaching to an order
 *  (the fab bytes stay in the store; only id/url/manifest travel). */
export function resolveArtifactRef(handle: string): ArtifactRef | null {
  const a = resolveArtifact(handle);
  if (!a) return null;
  return {
    artifact_id: a.id,
    artifact_url: artifactUrl(a.id),
    bytes: a.bytes,
    manifest: a.manifest,
  };
}

/** resolveArtifactRef with the durable fallback — quote_manufacturing /
 *  place_order use this so a handle minted by export_gerber on ANOTHER
 *  instance still binds to the order. */
export async function resolveArtifactRefAsync(
  handle: string,
): Promise<ArtifactRef | null> {
  const a = await resolveArtifactAsync(handle);
  if (!a) return null;
  return {
    artifact_id: a.id,
    artifact_url: artifactUrl(a.id),
    bytes: a.bytes,
    manifest: a.manifest,
  };
}

/** Extract the artifact id from a raw id or an artifact URL. */
export function parseArtifactId(handle: string): string | null {
  if (!handle) return null;
  if (registry.has(handle)) return handle;
  try {
    const parts = new URL(handle).pathname.split("/").filter(Boolean);
    const i = parts.indexOf("artifacts");
    if (i >= 0 && parts[i + 1]) return decodeURIComponent(parts[i + 1]);
  } catch {
    // not a URL — fall through
  }
  // Bare "art_…" id or a "/artifacts/<id>" path fragment.
  const seg = handle.split("/").filter(Boolean);
  const ai = seg.indexOf("artifacts");
  if (ai >= 0 && seg[ai + 1]) return decodeURIComponent(seg[ai + 1]);
  return handle.startsWith("art_") ? handle : null;
}

/** Test seam: clear the registry between cases. */
export function clearArtifacts(): void {
  registry.clear();
}
