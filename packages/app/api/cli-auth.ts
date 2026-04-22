/**
 * Device-code browser flow endpoint for `vcad login`.
 *
 * Two methods on the same route:
 *
 * - `POST /api/cli-auth` — called by the browser at /cli-auth after the
 *   user signs in. Requires a valid Supabase Bearer token. Body:
 *   `{ code, access_token, refresh_token?, expires_at? }`. Stores the
 *   token in the `cli_auth_codes` table, keyed by the one-time code.
 *
 * - `GET /api/cli-auth?code=X` — called by the TUI's polling loop.
 *   Returns 200 with the stored token (and deletes the row), 408 if
 *   the code exists but hasn't been filled in yet, or 404 if the code
 *   was never seen. The TUI treats 404/408 as "keep polling".
 *
 * Codes expire after 10 minutes — expired rows are ignored and cleaned
 * up lazily on each GET. The table migration lives at
 * `supabase/migrations/009_cli_auth_codes.sql`.
 */

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { applyCors, getSupabaseAdmin, getUserIdFromAuth } from "./_lib/supabase.js";

const CODE_TTL_SECONDS = 10 * 60;

function setCors(res: VercelResponse, req: VercelRequest): void {
  applyCors(res, req);
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
}

export default async function handler(
  req: VercelRequest,
  res: VercelResponse,
): Promise<void> {
  setCors(res, req);
  if (req.method === "OPTIONS") {
    res.status(204).end();
    return;
  }

  const admin = getSupabaseAdmin();
  if (!admin) {
    res.status(500).json({ error: "cli-auth: Supabase not configured" });
    return;
  }

  if (req.method === "POST") {
    await handlePost(req, res, admin);
    return;
  }
  if (req.method === "GET") {
    await handleGet(req, res, admin);
    return;
  }
  res.status(405).json({ error: "Method not allowed" });
}

async function handlePost(
  req: VercelRequest,
  res: VercelResponse,
  admin: ReturnType<typeof getSupabaseAdmin>,
): Promise<void> {
  // Must be signed in — the whole point is forwarding the caller's token.
  const userId = await getUserIdFromAuth(req, admin);
  if (!userId) {
    res.status(401).json({ error: "Unauthorized" });
    return;
  }

  const body = req.body as {
    code?: string;
    access_token?: string;
    refresh_token?: string | null;
    expires_at?: number | null;
  };
  const code = (body.code ?? "").trim();
  const accessToken = (body.access_token ?? "").trim();
  if (!code || !accessToken) {
    res.status(400).json({ error: "code and access_token are required" });
    return;
  }
  if (code.length > 64) {
    res.status(400).json({ error: "code too long" });
    return;
  }

  const now = Math.floor(Date.now() / 1000);
  const { error } = await admin!.from("cli_auth_codes").upsert({
    code,
    user_id: userId,
    access_token: accessToken,
    refresh_token: body.refresh_token ?? null,
    expires_at: body.expires_at ?? null,
    created_at: new Date(now * 1000).toISOString(),
  });
  if (error) {
    console.error("[cli-auth] insert failed:", error);
    res.status(500).json({ error: "failed to store code" });
    return;
  }

  res.status(200).json({ ok: true });
}

async function handleGet(
  req: VercelRequest,
  res: VercelResponse,
  admin: ReturnType<typeof getSupabaseAdmin>,
): Promise<void> {
  const code = (req.query.code as string | undefined)?.trim();
  if (!code) {
    res.status(400).json({ error: "code query parameter required" });
    return;
  }

  const { data, error } = await admin!
    .from("cli_auth_codes")
    .select("access_token, refresh_token, expires_at, created_at")
    .eq("code", code)
    .maybeSingle();

  if (error) {
    console.error("[cli-auth] lookup failed:", error);
    res.status(500).json({ error: "lookup failed" });
    return;
  }
  if (!data) {
    // Not yet populated by the browser flow; TUI keeps polling.
    res.status(408).json({ error: "pending" });
    return;
  }

  // TTL guard — ignore rows older than CODE_TTL_SECONDS.
  const createdAt = new Date(data.created_at as string).getTime() / 1000;
  if (Math.floor(Date.now() / 1000) - createdAt > CODE_TTL_SECONDS) {
    await admin!.from("cli_auth_codes").delete().eq("code", code);
    res.status(404).json({ error: "code expired" });
    return;
  }

  // One-time use — delete after successful read.
  await admin!.from("cli_auth_codes").delete().eq("code", code);

  res.status(200).json({
    access_token: data.access_token,
    refresh_token: data.refresh_token,
    expires_at: data.expires_at,
  });
}
