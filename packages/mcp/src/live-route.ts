/**
 * Shared HTTP handler for the live review window (`/live/*`).
 *
 * Both deployment entry points use it, so the routes behave identically:
 *   - services/mcp/entry.ts  → the Vercel serverless function (mcp.vcad.io)
 *   - packages/mcp/src/http.ts → the standalone Node server (Fly.io / local)
 *
 * Capability-keyed by the session id in the path — possession of the
 * unguessable id is the grant, same model as mcp_sessions. Reads/appends go
 * through the service-role event store, so they work for both anon and
 * signed-in sessions by session id alone. FLAG-GATED behind VCAD_LIVE_WINDOW
 * (default OFF → 404).
 *
 *   GET  /live/<id>/events[?since=N]  → the session's spine events (replay)
 *   POST /live/<id>/annotate          → append a viewer overlay (pin/flag/…)
 *
 * The geometry stream (GLB / fold) and the browser viewer app are a separate,
 * env-gated slice — this is the data backbone the broadcast trigger feeds.
 */

import type { IncomingMessage, ServerResponse } from "node:http";
import type { AuthUser } from "./oauth.js";
import {
  createSessionEventStore,
  createShareStore,
  resolveSessionIr,
} from "./session-store.js";
import { appendOverlay, listEvents } from "./tools/live.js";
import { generateGlbPreview } from "./tools/preview.js";
import { LIVE_HTML } from "./live-html.generated.js";
import type { Engine } from "@vcad/engine";

// Public Supabase creds for the browser viewer — the publishable anon key is
// designed to ship in client bundles. The service-role key is NEVER sent.
const PUBLIC_SUPABASE_URL_DEFAULT = "https://yteuhwciuxcbjwmabawj.supabase.co";
const PUBLIC_ANON_KEY_DEFAULT = "sb_publishable_pt2xNsK8d7fEbdlkj9PQrA_KvYERtjM";

/** An overlay body is tiny — a far smaller cap than the 10 MiB /mcp default. */
const LIVE_BODY_MAX_BYTES = 16 * 1024;

// ── Per-IP rate limiter (per-instance; weak on serverless but a real brake on
//    the standalone server, and a cheap floor everywhere). ──
const RATE_LIMIT_PER_MINUTE = parseInt(
  process.env.MCP_RATE_LIMIT_PER_MINUTE || "60",
  10,
);
const RATE_WINDOW_MS = 60_000;
const rateMap = new Map<string, { windowStart: number; count: number }>();

function clientIp(req: IncomingMessage): string {
  const fwd = req.headers["x-forwarded-for"];
  if (typeof fwd === "string" && fwd.length > 0) return fwd.split(",")[0].trim();
  return req.socket?.remoteAddress ?? "unknown";
}

function rateLimited(ip: string): boolean {
  if (RATE_LIMIT_PER_MINUTE <= 0) return false;
  const now = Date.now();
  const entry = rateMap.get(ip);
  if (!entry || now - entry.windowStart >= RATE_WINDOW_MS) {
    rateMap.set(ip, { windowStart: now, count: 1 });
    return false;
  }
  entry.count += 1;
  return entry.count > RATE_LIMIT_PER_MINUTE;
}

function readBody(req: IncomingMessage, maxBytes: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let total = 0;
    req.on("data", (chunk: Buffer) => {
      total += chunk.length;
      if (total > maxBytes) {
        reject(new Error("body too large"));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf-8")));
    req.on("error", reject);
  });
}

const text = (res: ServerResponse, status: number, body: string): void => {
  res.writeHead(status, { "Content-Type": "text/plain" });
  res.end(body);
};
const json = (res: ServerResponse, status: number, body: unknown): void => {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
};

/**
 * Handle a `/live/*` request. Returns `true` once it has written a response —
 * callers do `if (await handleLiveRequest(req, res, { user })) return;`. Returns
 * `false` for any non-`/live` path so the caller continues its own routing.
 */
export async function handleLiveRequest(
  req: IncomingMessage,
  res: ServerResponse,
  opts: { user: AuthUser | null; getEngine?: () => Promise<Engine> } = { user: null },
): Promise<boolean> {
  const url = new URL(req.url ?? "/", `https://${req.headers.host ?? "localhost"}`);
  if (!url.pathname.startsWith("/live/")) return false;

  // A new public read/append surface — off unless explicitly enabled.
  if (process.env.VCAD_LIVE_WINDOW !== "1") {
    text(res, 404, "Not Found");
    return true;
  }

  const parts = url.pathname.split("/").filter(Boolean); // ["live", id, action]
  const sessionId = parts[1] ? decodeURIComponent(parts[1]) : "";
  const action = parts[2] ?? "";
  if (!sessionId) {
    text(res, 400, "missing session id");
    return true;
  }

  // Bound the per-IP rate of EVERY /live route with one early check — the gate
  // query, the HTML page, config, replay, annotate, and glb all pass through.
  if (rateLimited(clientIp(req))) {
    text(res, 429, "Too Many Requests");
    return true;
  }

  // Private by default: the session must be explicitly shared (share_session
  // wrote a live_shares row) or every /live route 404s — even with the flag on
  // and a valid id. The share record's owner scopes geometry resolution so a
  // link-holder can only ever see the actual sharer's document.
  const share = await createShareStore().getShare(sessionId);
  if (!share) {
    text(res, 404, "Not Found");
    return true;
  }

  // The viewer page (GET /live/<id>) — served only for a shared session.
  if (req.method === "GET" && action === "") {
    res.writeHead(200, {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-store",
    });
    res.end(LIVE_HTML);
    return true;
  }

  // Public realtime config for the browser app — anon/publishable key only.
  if (req.method === "GET" && action === "config") {
    json(res, 200, {
      session_id: sessionId,
      supabaseUrl: (process.env.SUPABASE_URL || PUBLIC_SUPABASE_URL_DEFAULT).replace(/\/+$/, ""),
      anonKey: process.env.SUPABASE_ANON_KEY || PUBLIC_ANON_KEY_DEFAULT,
    });
    return true;
  }

  const eventStore = createSessionEventStore(opts.user);

  if (req.method === "GET" && action === "events") {
    const sinceRaw = url.searchParams.get("since");
    const since = sinceRaw != null && sinceRaw !== "" ? Number(sinceRaw) : undefined;
    const events = await listEvents(
      eventStore,
      sessionId,
      Number.isFinite(since) ? since : undefined,
    );
    json(res, 200, { session_id: sessionId, events });
    return true;
  }

  if (req.method === "POST" && action === "annotate") {
    let parsed: unknown;
    try {
      parsed = JSON.parse(await readBody(req, LIVE_BODY_MAX_BYTES));
    } catch {
      json(res, 400, { ok: false, error: "invalid or oversized json body" });
      return true;
    }
    // Tolerate null / array / scalar bodies — appendOverlay rejects the empty
    // overlay with a clean 400 instead of throwing a 500.
    const body =
      parsed && typeof parsed === "object" && !Array.isArray(parsed)
        ? (parsed as Record<string, unknown>)
        : {};
    const result = await appendOverlay(
      eventStore,
      sessionId,
      {
        type: typeof body.type === "string" ? body.type : "",
        payload: body.payload,
        author: typeof body.author === "string" ? body.author : undefined,
      },
      // The verified token identity is authoritative; a body author is only ever
      // honored (namespaced) for genuinely anonymous viewers.
      { trustedAuthor: opts.user?.email || undefined },
    );
    json(res, result.ok ? 200 : 400, result);
    return true;
  }

  if (req.method === "GET" && action === "glb") {
    // Geometry for a hostless viewer that knows only the capability id: resolve
    // the session IR by id (service role), scoped to the sharer so a link-holder
    // can't be served a spoofed document. The session is already share-gated.
    const doc = await resolveSessionIr(sessionId, share.shared_by);
    if (!doc) {
      text(res, 404, "Not Found");
      return true;
    }
    const engine = opts.getEngine ? await opts.getEngine() : undefined;
    if (!engine) {
      text(res, 503, "geometry engine unavailable");
      return true;
    }
    const b64 = await generateGlbPreview(doc, engine);
    if (!b64) {
      text(res, 404, "no previewable geometry");
      return true;
    }
    const bytes = Buffer.from(b64, "base64");
    res.writeHead(200, {
      "Content-Type": "model/gltf-binary",
      "Cache-Control": "no-store",
      "Content-Length": String(bytes.length),
    });
    res.end(bytes);
    return true;
  }

  text(res, 404, "Not Found");
  return true;
}
