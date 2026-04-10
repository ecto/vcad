import { createClient, type SupabaseClient } from "@supabase/supabase-js";

// Supabase client - only created if credentials are configured
let supabaseClient: SupabaseClient | null = null;

function getSupabaseCredentials(): { url: string; anonKey: string } | null {
  // Support both Vite and Node environments
  const url =
    (typeof import.meta !== "undefined" &&
      (import.meta as { env?: Record<string, string> }).env
        ?.VITE_SUPABASE_URL) ||
    (typeof process !== "undefined" && process.env?.SUPABASE_URL);

  const anonKey =
    (typeof import.meta !== "undefined" &&
      (import.meta as { env?: Record<string, string> }).env
        ?.VITE_SUPABASE_ANON_KEY) ||
    (typeof process !== "undefined" && process.env?.SUPABASE_ANON_KEY);

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
