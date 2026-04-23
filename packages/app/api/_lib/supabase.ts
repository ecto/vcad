// Shared Supabase admin client + auth header parsing for Vercel functions.
//
// Files under api/_lib are not deployed as endpoints — Vercel ignores
// underscore-prefixed paths under api/. They're plain helper modules that
// the real endpoint files import.

import type { VercelRequest } from "@vercel/node";
import { createClient, type SupabaseClient } from "@supabase/supabase-js";

let cached: SupabaseClient | null = null;

/** Lazily construct (and cache) the service-role Supabase client. Returns
 *  null if credentials are missing so self-hosted deployments can run without
 *  billing/auth wiring. */
export function getSupabaseAdmin(): SupabaseClient | null {
  if (cached) return cached;
  const url = process.env.SUPABASE_URL;
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY;
  if (!url || !key) return null;
  cached = createClient(url, key, { auth: { persistSession: false } });
  return cached;
}

/** Extract the authenticated user id from a request's Bearer token, or null
 *  if the token is missing / invalid.
 *
 *  By default, anonymous Supabase sessions (`user.is_anonymous`) are treated
 *  as not-signed-in (returns null) so existing rate-limit / entitlement code
 *  paths still apply the anonymous tier rules. Pass `{ allowAnon: true }`
 *  when you want the actual uid back (e.g. to scope rows in chat_threads). */
export async function getUserIdFromAuth(
  req: VercelRequest,
  admin: SupabaseClient | null,
  opts: { allowAnon?: boolean } = {},
): Promise<string | null> {
  if (!admin) return null;
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith("Bearer ")) return null;
  const token = authHeader.slice(7);
  try {
    const { data, error } = await admin.auth.getUser(token);
    if (error || !data.user) return null;
    if (data.user.is_anonymous && !opts.allowAnon) return null;
    return data.user.id;
  } catch {
    return null;
  }
}

/** Result of resolving the Bearer token on a request.
 *
 *  `tokenStatus` lets callers distinguish three cases that are otherwise
 *  collapsed into "userId is null":
 *    * `"missing"` — no Authorization header (a true anonymous caller — e.g.
 *      the marketing site, a CLI without `vcad login`, the `/api/chat` flow
 *      that explicitly supports anonymous use).
 *    * `"valid"`   — a token was sent and Supabase accepted it. `userId` is
 *      populated with the resolved auth.uid().
 *    * `"invalid"` — a token was sent but Supabase rejected it (expired,
 *      malformed, revoked, network blip on getUser). The caller should
 *      respond with 401 so the client can refresh its session and retry,
 *      rather than silently downgrading the request to the anonymous tier
 *      and producing a misleading "free chat limit reached" error. */
export interface AuthDetail {
  userId: string | null;
  isAnonymous: boolean;
  tokenStatus: "missing" | "valid" | "invalid";
}

/** Extract the uid, anonymity flag, and token-validation status from a
 *  request's Bearer token. See `AuthDetail` for the three-way distinction
 *  callers need when a logged-in user's token is rejected. */
export async function getAuthDetail(
  req: VercelRequest,
  admin: SupabaseClient | null,
): Promise<AuthDetail> {
  if (!admin) return { userId: null, isAnonymous: false, tokenStatus: "missing" };
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith("Bearer ")) {
    return { userId: null, isAnonymous: false, tokenStatus: "missing" };
  }
  const token = authHeader.slice(7);
  try {
    const { data, error } = await admin.auth.getUser(token);
    if (error || !data.user) {
      return { userId: null, isAnonymous: false, tokenStatus: "invalid" };
    }
    return {
      userId: data.user.id,
      isAnonymous: !!data.user.is_anonymous,
      tokenStatus: "valid",
    };
  } catch {
    return { userId: null, isAnonymous: false, tokenStatus: "invalid" };
  }
}

/** The request origin will only receive CORS headers if it appears in this
 *  allowlist. VCAD_CORS_ALLOWED_ORIGINS is a comma-separated env var; when
 *  unset, we default to the production web origin only. */
function allowedOrigins(): string[] {
  const env = process.env.VCAD_CORS_ALLOWED_ORIGINS;
  if (env && env.trim().length > 0) {
    return env.split(",").map((s) => s.trim()).filter(Boolean);
  }
  return ["https://vcad.io"];
}

/** Set CORS headers — but only for origins on the allowlist. For non-browser
 *  clients (Bearer-authed CLI, server-to-server), no Origin header is sent
 *  and CORS is not involved. Previously this used `*`, which let any web
 *  page on any origin call these endpoints with the user's access token. */
export function applyCors(
  res: { setHeader: (k: string, v: string) => void },
  req?: { headers: { origin?: string | string[] } },
): void {
  const origin = req && req.headers ? req.headers.origin : undefined;
  const originStr = Array.isArray(origin) ? origin[0] : origin;
  if (originStr && allowedOrigins().includes(originStr)) {
    res.setHeader("Access-Control-Allow-Origin", originStr);
    res.setHeader("Vary", "Origin");
  }
  res.setHeader("Access-Control-Allow-Methods", "POST, GET, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization, Stripe-Signature");
}
