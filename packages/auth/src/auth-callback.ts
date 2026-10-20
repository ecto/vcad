/**
 * Auth callback handling.
 *
 * `applyAuthCallback` parses whatever Supabase put on a callback URL (query
 * `?code=...` for PKCE, query `?token_hash=...&type=...` for the OTP
 * magic-link, or hash `#access_token=...&refresh_token=...` for the
 * implicit flow) and turns it into an active Supabase session. Used by the
 * web popup OAuth flow.
 */

import { getSupabase } from "./client";

export interface AuthCallbackResult {
  /** True when the URL produced an active session. */
  ok: boolean;
  /** Populated on failure; safe to surface to the user. */
  error?: string;
}

/**
 * Materialize a Supabase session from any auth callback URL — used by
 * the web popup flow. Handles
 * three shapes Supabase can return:
 *
 *   - `?code=…`            PKCE / OAuth code exchange
 *   - `#access_token=…`    implicit-flow token pair
 *   - `?token_hash=…&type` magic-link OTP
 *
 * Returns `{ ok: true }` when a session was established; `AuthProvider`
 * will pick it up via its `onAuthStateChange` subscription.
 */
export async function applyAuthCallback(
  url: string,
): Promise<AuthCallbackResult> {
  const supabase = getSupabase();
  if (!supabase) {
    return { ok: false, error: "Auth is not configured" };
  }

  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return { ok: false, error: "Malformed callback URL" };
  }

  // Implicit-flow tokens land in the fragment. Supabase exposes
  // `setSession` as the canonical way to install them on the client.
  const hash = parsed.hash.startsWith("#") ? parsed.hash.slice(1) : parsed.hash;
  if (hash) {
    const fragment = new URLSearchParams(hash);
    const accessToken = fragment.get("access_token");
    const refreshToken = fragment.get("refresh_token");
    if (accessToken && refreshToken) {
      const { error } = await supabase.auth.setSession({
        access_token: accessToken,
        refresh_token: refreshToken,
      });
      if (error) return { ok: false, error: error.message };
      return { ok: true };
    }
  }

  const query = parsed.searchParams;

  // PKCE / OAuth code exchange. Note this only succeeds when the
  // exchanging client is the same one that initiated the sign-in — the
  // code verifier lives in the app's localStorage.
  const code = query.get("code");
  if (code) {
    const { error } = await supabase.auth.exchangeCodeForSession(code);
    if (error) return { ok: false, error: error.message };
    return { ok: true };
  }

  // Magic-link OTP: token_hash is single-use and verified server-side,
  // so it works even though the click happened in the user's browser.
  // `signInWithOtp` always emits one of the email OTP types (magiclink
  // by default, or `email` when shouldCreateUser is false). The
  // `EmailOtpType` union is not exported, so we narrow with a cast on
  // the verified-against-the-server set.
  const tokenHash = query.get("token_hash");
  const otpType = query.get("type") as
    | "signup"
    | "invite"
    | "magiclink"
    | "recovery"
    | "email_change"
    | "email"
    | null;
  if (tokenHash && otpType) {
    const { error } = await supabase.auth.verifyOtp({
      token_hash: tokenHash,
      type: otpType,
    });
    if (error) return { ok: false, error: error.message };
    return { ok: true };
  }

  const errDescription =
    query.get("error_description") ?? query.get("error") ?? null;
  if (errDescription) {
    return { ok: false, error: errDescription };
  }

  return { ok: false, error: "Callback URL had no auth parameters" };
}
