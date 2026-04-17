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

/** Extract both the uid and whether the session is anonymous. Use this
 *  when you need to differentiate between "no session", "anon session",
 *  and "permanent session" — for example, the chat endpoint stores rows
 *  under the anon uid but applies the anonymous rate-limit tier. */
export async function getAuthDetail(
  req: VercelRequest,
  admin: SupabaseClient | null,
): Promise<{ userId: string | null; isAnonymous: boolean }> {
  if (!admin) return { userId: null, isAnonymous: false };
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith("Bearer ")) {
    return { userId: null, isAnonymous: false };
  }
  const token = authHeader.slice(7);
  try {
    const { data, error } = await admin.auth.getUser(token);
    if (error || !data.user) return { userId: null, isAnonymous: false };
    return { userId: data.user.id, isAnonymous: !!data.user.is_anonymous };
  } catch {
    return { userId: null, isAnonymous: false };
  }
}

/** Set the CORS headers this app uses on every endpoint. */
export function applyCors(res: { setHeader: (k: string, v: string) => void }): void {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "POST, GET, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization, Stripe-Signature");
}
