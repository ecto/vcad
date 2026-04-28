/**
 * True when running inside the Tauri desktop shell. Detected via the
 * runtime global the @tauri-apps/api `isTauri()` helper checks for, so
 * we don't need to take a hard dep on tauri from the auth package
 * (which is also consumed by the pure-web bundle).
 */
export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  return "__TAURI_INTERNALS__" in window;
}

/**
 * Where Supabase should redirect the magic-link click. On web this is
 * the current origin (Supabase strips its tokens out of the URL via
 * `detectSessionInUrl`). On desktop the magic-link arrives in the
 * user's external browser, which can't hand tokens back to the Tauri
 * window directly — so we route through a static bridge page on
 * vcad.io that JS-redirects to the `vcad://auth/callback` deep link.
 *
 * The bridge page does NOT load supabase-js (which would auto-verify
 * the one-shot token_hash and prevent the desktop from completing the
 * sign-in). It only forwards the URL's query + fragment unchanged, so
 * the desktop can call `verifyOtp` / `setSession` itself.
 */
export function getAuthRedirectUrl(): string {
  if (isTauriRuntime()) {
    return "https://vcad.io/auth/desktop";
  }
  return typeof window !== "undefined" ? window.location.origin : "";
}

/**
 * Where Supabase should redirect a popup OAuth flow. This is a static
 * page that postMessages the callback URL to its opener and closes
 * itself, so the main window completes the PKCE exchange against the
 * existing Supabase client without ever navigating away.
 *
 * Web only — Tauri keeps using `getAuthRedirectUrl()` so the OS browser
 * routes back through the deep-link bridge.
 */
export function getPopupCallbackUrl(): string {
  if (typeof window === "undefined") return "";
  // `.html` extension keeps the URL pointing at the static file even
  // on hosts that fall back to the SPA's index.html for unknown paths
  // (Vite dev server, Cloudflare Pages, Vercel, etc.).
  return `${window.location.origin}/auth/popup.html`;
}

import {
  createClient,
  type Session as SupabaseSession,
  type SupabaseClient,
} from "@supabase/supabase-js";

// Supabase client - only created if credentials are configured
let supabaseClient: SupabaseClient | null = null;

// Production defaults. The `sb_publishable_*` key is a Supabase publishable
// key — explicitly designed to be embedded in client bundles (web, mobile,
// desktop). Row Level Security enforces authorization server-side, so
// shipping these values in the binary is safe and is what Supabase recommends
// for end-user distribution. Env vars override for staging / local work.
const DEFAULT_SUPABASE_URL = "https://yteuhwciuxcbjwmabawj.supabase.co";
const DEFAULT_SUPABASE_ANON_KEY =
  "sb_publishable_pt2xNsK8d7fEbdlkj9PQrA_KvYERtjM";

function getSupabaseCredentials(): { url: string; anonKey: string } | null {
  // Support both Vite and Node environments
  const url =
    (typeof import.meta !== "undefined" &&
      (import.meta as { env?: Record<string, string> }).env
        ?.VITE_SUPABASE_URL) ||
    (typeof process !== "undefined" && process.env?.SUPABASE_URL) ||
    DEFAULT_SUPABASE_URL;

  const anonKey =
    (typeof import.meta !== "undefined" &&
      (import.meta as { env?: Record<string, string> }).env
        ?.VITE_SUPABASE_ANON_KEY) ||
    (typeof process !== "undefined" && process.env?.SUPABASE_ANON_KEY) ||
    DEFAULT_SUPABASE_ANON_KEY;

  if (!url || !anonKey) {
    return null;
  }

  return { url, anonKey };
}

/**
 * Get or create the Supabase client.
 * Returns null if credentials are not configured.
 */
let _supabaseWarned = false;

export function getSupabase(): SupabaseClient | null {
  if (supabaseClient) {
    return supabaseClient;
  }

  const credentials = getSupabaseCredentials();
  if (!credentials) {
    if (!_supabaseWarned) {
      console.warn("Supabase credentials not configured - auth features disabled");
      _supabaseWarned = true;
    }
    return null;
  }

  supabaseClient = createClient(credentials.url, credentials.anonKey, {
    auth: {
      persistSession: true,
      autoRefreshToken: true,
      detectSessionInUrl: true,
    },
  });

  return supabaseClient;
}

/**
 * Check if authentication is enabled.
 * Auth is disabled when Supabase credentials are not configured (e.g., self-hosted).
 */
export function isAuthEnabled(): boolean {
  return getSupabaseCredentials() !== null;
}

/**
 * Get the Supabase client, throwing if not available.
 * Use this in contexts where auth is required.
 */
export function requireSupabase(): SupabaseClient {
  const client = getSupabase();
  if (!client) {
    throw new Error("Supabase not configured");
  }
  return client;
}

/**
 * Ensure the current Supabase client has a session — sign in anonymously if
 * not. Returns the active session, or null when Supabase is not configured.
 *
 * Anonymous sessions give the user a real `auth.uid()` so RLS predicates of
 * the form `auth.uid() = user_id` work uniformly across anon and authed
 * users. When the user later signs in with Google/GitHub, Supabase emits a
 * USER_UPDATED event with a new uid, and the AuthProvider re-parents any
 * rows owned by the previous anon uid.
 *
 * Concurrent callers are deduped via an in-flight promise — `signInAnonymously`
 * is not idempotent on the wire and we don't want two anon users created on
 * a race.
 */
let _ensureSessionInflight: Promise<SupabaseSession | null> | null = null;

export async function ensureSession(): Promise<SupabaseSession | null> {
  const client = getSupabase();
  if (!client) return null;

  const { data } = await client.auth.getSession();
  if (data.session) return data.session;

  if (_ensureSessionInflight) return _ensureSessionInflight;

  _ensureSessionInflight = (async () => {
    try {
      const { data: anon, error } = await client.auth.signInAnonymously();
      if (error) {
        console.error("[auth] anonymous sign-in failed:", error.message);
        return null;
      }
      return anon.session;
    } finally {
      _ensureSessionInflight = null;
    }
  })();

  return _ensureSessionInflight;
}
