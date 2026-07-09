/**
 * Shared HTTP handler for the artifact channel (`/artifacts/*`).
 *
 * Both deployment entry points use it, so the routes behave identically:
 *   - services/mcp/entry.ts   → the Vercel serverless function (mcp.vcad.io)
 *   - packages/mcp/src/http.ts → the standalone Node server (Fly.io / local)
 *
 * Capability-keyed by the unguessable artifact id in the path — possession of
 * the id is the grant, the same model as live-share session ids. This is how a
 * large fab/export bundle leaves the model's context: the tool returns a
 * { artifact_url, manifest } handle and the actual bytes are fetched here.
 *
 *   GET /artifacts/<id>          → the bundle manifest (JSON index)
 *   GET /artifacts/<id>/<file>   → one file's raw bytes
 */

import type { IncomingMessage, ServerResponse } from "node:http";
import {
  getArtifactAsync,
  getArtifactFileAsync,
  artifactFileUrl,
} from "./tools/artifact-store.js";

const text = (res: ServerResponse, status: number, body: string): void => {
  res.writeHead(status, { "Content-Type": "text/plain" });
  res.end(body);
};
const json = (res: ServerResponse, status: number, body: unknown): void => {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
};

/**
 * Handle an `/artifacts/*` request. Returns `true` once it has written a
 * response — callers do `if (await handleArtifactRequest(req, res)) return;`.
 * Returns `false` for any non-`/artifacts` path so the caller keeps routing.
 */
export async function handleArtifactRequest(
  req: IncomingMessage,
  res: ServerResponse,
): Promise<boolean> {
  const url = new URL(req.url ?? "/", `https://${req.headers.host ?? "localhost"}`);
  if (!url.pathname.startsWith("/artifacts/")) return false;

  if (req.method !== "GET" && req.method !== "HEAD") {
    text(res, 405, "Method Not Allowed");
    return true;
  }

  const parts = url.pathname.split("/").filter(Boolean); // ["artifacts", id, ...file]
  const id = parts[1] ? decodeURIComponent(parts[1]) : "";
  const fileName = parts
    .slice(2)
    .map((p) => decodeURIComponent(p))
    .join("/");

  if (!id) {
    text(res, 400, "missing artifact id");
    return true;
  }

  const artifact = await getArtifactAsync(id);
  if (!artifact) {
    text(res, 404, "Not Found");
    return true;
  }

  // Index: the manifest plus a per-file download URL.
  if (!fileName) {
    json(res, 200, {
      artifact_id: id,
      bytes: artifact.bytes,
      expires_at: new Date(artifact.expiresAt).toISOString(),
      manifest: artifact.manifest,
      files: artifact.manifest.map((m) => ({
        file: m.file,
        bytes: m.bytes,
        sha256: m.sha256,
        url: artifactFileUrl(id, m.file),
      })),
    });
    return true;
  }

  const file = await getArtifactFileAsync(id, fileName);
  if (!file) {
    text(res, 404, "Not Found");
    return true;
  }

  res.writeHead(200, {
    "Content-Type": file.contentType,
    "Content-Length": String(file.buf.length),
    "Cache-Control": "private, max-age=86400",
  });
  res.end(req.method === "HEAD" ? undefined : file.buf);
  return true;
}
