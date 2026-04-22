/**
 * Device-code browser flow endpoint for `vcad login`.
 *
 * Two methods on the same route:
 *
 * - `POST /api/cli-auth` — called by the browser at /cli-auth after the
 *   user signs in. Requires a valid Supabase Bearer token. Body:
 *   `{ code, access_token, refresh_token?, expires_at? }`. Stores the
 *   token in the `cli_auth_codes` table, keyed by a salted hash of the
 *   one-time code, encrypted with a key derived from the code itself.
 *
 * - `GET /api/cli-auth?code=X` — called by the TUI's polling loop.
 *   Returns 200 with the plaintext token (and deletes the row), 408 if
 *   the code exists but hasn't been filled in yet, or 404 if the code
 *   was never seen. The TUI treats 404/408 as "keep polling".
 *
 * Codes expire after 10 minutes — expired rows are ignored and cleaned
 * up lazily on each GET. The table migration lives at
 * `supabase/migrations/009_cli_auth_codes.sql` and
 * `supabase/migrations/017_cli_auth_codes_encrypted.sql`.
 *
 * Required environment variables in production:
 *   CLI_AUTH_CODE_PEPPER  - secret string mixed into the code hash so a
 *                          database dump alone cannot be used to guess
 *                          codes offline.
 *   CLI_AUTH_ENC_KEY      - 32-byte key (hex, 64 chars) for HKDF/AES-GCM.
 *
 * The endpoint refuses to serve if either is missing.
 */

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { applyCors, getSupabaseAdmin, getUserIdFromAuth } from "./_lib/supabase.js";
import { createHmac, randomBytes, createCipheriv, createDecipheriv, hkdfSync } from "node:crypto";

const CODE_TTL_SECONDS = 10 * 60;
const MAX_CODE_LEN = 64;
// Per-user rate limit for POST (issuing codes). 10 requests / minute is
// plenty for legitimate re-auth attempts and stops a compromised session
// from flooding the table.
const POST_LIMIT_PER_MINUTE = 10;

function setCors(res: VercelResponse, req: VercelRequest): void {
  applyCors(res, req);
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
}

function secrets(): { pepper: string; encKey: Buffer } | null {
  const pepper = process.env.CLI_AUTH_CODE_PEPPER;
  const encKeyHex = process.env.CLI_AUTH_ENC_KEY;
  if (!pepper || pepper.length < 16) return null;
  if (!encKeyHex || encKeyHex.length !== 64) return null;
  let encKey: Buffer;
  try {
    encKey = Buffer.from(encKeyHex, "hex");
  } catch {
    return null;
  }
  if (encKey.length !== 32) return null;
  return { pepper, encKey };
}

function hashCode(code: string, pepper: string): string {
  // HMAC-SHA256 with the pepper as key gives a keyed hash that is stable
  // across requests but not precomputable from the database alone.
  return createHmac("sha256", pepper).update(code).digest("hex");
}

function deriveCipherKey(encKey: Buffer, lookupKey: string): Buffer {
  const out = hkdfSync("sha256", encKey, Buffer.from(lookupKey, "hex"), Buffer.from("vcad-cli-auth-v1"), 32);
  return Buffer.from(out);
}

function encryptToken(encKey: Buffer, lookupKey: string, plaintext: string): { nonce: Buffer; ciphertext: Buffer } {
  const nonce = randomBytes(12);
  const key = deriveCipherKey(encKey, lookupKey);
  const cipher = createCipheriv("aes-256-gcm", key, nonce);
  const enc = Buffer.concat([cipher.update(plaintext, "utf8"), cipher.final()]);
  const tag = cipher.getAuthTag();
  return { nonce, ciphertext: Buffer.concat([enc, tag]) };
}

function decryptToken(encKey: Buffer, lookupKey: string, nonce: Buffer, ciphertext: Buffer): string {
  const key = deriveCipherKey(encKey, lookupKey);
  const decipher = createDecipheriv("aes-256-gcm", key, nonce);
  const tag = ciphertext.subarray(ciphertext.length - 16);
  const enc = ciphertext.subarray(0, ciphertext.length - 16);
  decipher.setAuthTag(tag);
  return Buffer.concat([decipher.update(enc), decipher.final()]).toString("utf8");
}

// In-memory per-user POST counter. Vercel functions are stateless across
// warm invocations, so this only catches same-instance bursts — it's a
// defense-in-depth for a specific abuse pattern, not a hard quota.
const postCounters = new Map<string, { windowStart: number; count: number }>();
function tooManyPosts(userId: string): boolean {
  const now = Date.now();
  const entry = postCounters.get(userId);
  if (!entry || now - entry.windowStart >= 60_000) {
    postCounters.set(userId, { windowStart: now, count: 1 });
    return false;
  }
  entry.count += 1;
  return entry.count > POST_LIMIT_PER_MINUTE;
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
  const s = secrets();
  if (!s) {
    res.status(500).json({ error: "cli-auth: CLI_AUTH_CODE_PEPPER or CLI_AUTH_ENC_KEY missing" });
    return;
  }

  if (req.method === "POST") {
    await handlePost(req, res, admin, s);
    return;
  }
  if (req.method === "GET") {
    await handleGet(req, res, admin, s);
    return;
  }
  res.status(405).json({ error: "Method not allowed" });
}

async function handlePost(
  req: VercelRequest,
  res: VercelResponse,
  admin: NonNullable<ReturnType<typeof getSupabaseAdmin>>,
  s: { pepper: string; encKey: Buffer },
): Promise<void> {
  // Must be signed in — the whole point is forwarding the caller's token.
  const userId = await getUserIdFromAuth(req, admin);
  if (!userId) {
    res.status(401).json({ error: "Unauthorized" });
    return;
  }
  if (tooManyPosts(userId)) {
    res.status(429).json({ error: "Too many requests" });
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
  if (code.length > MAX_CODE_LEN) {
    res.status(400).json({ error: "code too long" });
    return;
  }
  // Reject low-entropy codes outright. A real browser-flow code is >=16
  // random bytes; anything shorter would let an attacker precompute the
  // lookup hash for trivially small code spaces.
  if (code.length < 16) {
    res.status(400).json({ error: "code too short" });
    return;
  }

  const lookupKey = hashCode(code, s.pepper);
  // Pack both tokens into a single AES-GCM ciphertext so we only need one
  // nonce and a single auth tag covers the whole payload.
  const payload = JSON.stringify({
    access_token: accessToken,
    refresh_token: body.refresh_token ?? null,
  });
  const enc = encryptToken(s.encKey, lookupKey, payload);

  const now = Math.floor(Date.now() / 1000);
  const { error } = await admin.from("cli_auth_codes").upsert({
    code: lookupKey,
    user_id: userId,
    access_token: null,
    refresh_token: null,
    enc_access_token: enc.ciphertext,
    enc_refresh_token: null,
    enc_nonce: enc.nonce,
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
  admin: NonNullable<ReturnType<typeof getSupabaseAdmin>>,
  s: { pepper: string; encKey: Buffer },
): Promise<void> {
  const code = (req.query.code as string | undefined)?.trim();
  if (!code || code.length > MAX_CODE_LEN) {
    res.status(400).json({ error: "code query parameter required" });
    return;
  }
  const lookupKey = hashCode(code, s.pepper);

  const { data, error } = await admin
    .from("cli_auth_codes")
    .select("enc_access_token, enc_refresh_token, enc_nonce, expires_at, created_at")
    .eq("code", lookupKey)
    .maybeSingle();

  if (error) {
    console.error("[cli-auth] lookup failed:", error);
    res.status(500).json({ error: "lookup failed" });
    return;
  }
  if (!data || !data.enc_access_token || !data.enc_nonce) {
    // Not yet populated by the browser flow; TUI keeps polling.
    res.status(408).json({ error: "pending" });
    return;
  }

  // TTL guard — ignore rows older than CODE_TTL_SECONDS.
  const createdAt = new Date(data.created_at as string).getTime() / 1000;
  if (Math.floor(Date.now() / 1000) - createdAt > CODE_TTL_SECONDS) {
    await admin.from("cli_auth_codes").delete().eq("code", lookupKey);
    res.status(404).json({ error: "code expired" });
    return;
  }

  // One-time use — delete after successful read.
  await admin.from("cli_auth_codes").delete().eq("code", lookupKey);

  try {
    const nonce = Buffer.from(data.enc_nonce as unknown as ArrayBufferLike);
    const enc = Buffer.from(data.enc_access_token as unknown as ArrayBufferLike);
    const plaintext = decryptToken(s.encKey, lookupKey, nonce, enc);
    const tokens = JSON.parse(plaintext) as {
      access_token: string;
      refresh_token: string | null;
    };
    res.status(200).json({
      access_token: tokens.access_token,
      refresh_token: tokens.refresh_token,
      expires_at: data.expires_at,
    });
  } catch (err) {
    console.error("[cli-auth] decryption failed:", err);
    res.status(500).json({ error: "decryption failed" });
  }
}
