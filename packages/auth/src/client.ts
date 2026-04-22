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
