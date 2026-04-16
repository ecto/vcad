import { requireSupabase, isAuthEnabled } from "./client";
import { useAuthStore } from "./stores/auth-store";

export interface Profile {
  id: string;
  username: string;
  display_name: string | null;
  bio: string | null;
  avatar_url: string | null;
  created_at: string;
  updated_at: string;
}

export interface PublicDocumentResult {
  id: string;
  name: string;
  content: unknown;
  version: number;
  updated_at: string;
  owner_username: string;
  owner_display_name: string | null;
  owner_avatar_url: string | null;
}

export interface PublicDocumentMeta {
  id: string;
  name: string;
  slug: string;
  updated_at: string;
  published_at: string | null;
}

/**
 * Get the current user's profile. Returns null if no profile exists yet
 * (user hasn't picked a username).
 */
export async function getMyProfile(): Promise<Profile | null> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) return null;
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("profiles")
    .select("*")
    .eq("id", user.id)
    .maybeSingle();
  if (error) throw error;
  return data as Profile | null;
}

/**
 * Get a profile by username. Works for any user (public read).
 */
export async function getProfileByUsername(
  username: string,
): Promise<Profile | null> {
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("profiles")
    .select("*")
    .eq("username", username)
    .maybeSingle();
  if (error) throw error;
  return data as Profile | null;
}

/**
 * Check if a username is available. Returns true if no profile uses it.
 */
export async function checkUsernameAvailable(
  username: string,
): Promise<boolean> {
  const supabase = requireSupabase();
  const { count, error } = await supabase
    .from("profiles")
    .select("id", { count: "exact", head: true })
    .eq("username", username);
  if (error) throw error;
  return (count ?? 0) === 0;
}

/**
 * Create a profile for the current user. Called once when the user
 * picks their username for the first time.
 */
export async function createProfile(
  username: string,
  displayName?: string,
): Promise<Profile> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) {
    throw new Error("Must be signed in to create a profile");
  }
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("profiles")
    .insert({
      id: user.id,
      username,
      display_name: displayName ?? user.user_metadata?.full_name ?? null,
      avatar_url: user.user_metadata?.avatar_url ?? null,
    })
    .select("*")
    .single();
  if (error) throw error;
  return data as Profile;
}

/**
 * Update the current user's profile.
 */
export async function updateProfile(
  updates: Partial<Pick<Profile, "username" | "display_name" | "bio" | "avatar_url">>,
): Promise<Profile> {
  const { user } = useAuthStore.getState();
  if (!isAuthEnabled() || !user) {
    throw new Error("Must be signed in to update profile");
  }
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("profiles")
    .update(updates)
    .eq("id", user.id)
    .select("*")
    .single();
  if (error) throw error;
  return data as Profile;
}

/**
 * Fetch a public document by /@username/slug. Anonymous callers allowed.
 */
export async function fetchPublicDocument(
  username: string,
  slug: string,
): Promise<PublicDocumentResult | null> {
  const supabase = requireSupabase();
  const { data, error } = await supabase.rpc("get_public_document", {
    p_username: username,
    p_slug: slug,
  });
  if (error) {
    console.warn("[profile] fetchPublicDocument error:", error);
    return null;
  }
  if (!data || (Array.isArray(data) && data.length === 0)) return null;
  const row = Array.isArray(data) ? data[0] : data;
  return row as PublicDocumentResult;
}

/**
 * List public documents for a user's profile page.
 */
export async function listPublicDocuments(
  username: string,
): Promise<PublicDocumentMeta[]> {
  const supabase = requireSupabase();
  const { data, error } = await supabase.rpc("list_public_documents", {
    p_username: username,
  });
  if (error) {
    console.warn("[profile] listPublicDocuments error:", error);
    return [];
  }
  return (data ?? []) as PublicDocumentMeta[];
}

/**
 * Set a document's slug and visibility. Used when the owner first shares
 * a document publicly. Auto-generates a slug from the document name if
 * none is provided.
 */
export async function publishDocument(
  cloudDocId: string,
  slug: string,
  visibility: "public" | "unlisted" = "public",
): Promise<void> {
  if (!isAuthEnabled()) return;
  const supabase = requireSupabase();
  const { error } = await supabase
    .from("documents")
    .update({
      slug,
      visibility,
      published_at: new Date().toISOString(),
    })
    .eq("id", cloudDocId);
  if (error) throw error;
}

/**
 * Generate a URL-safe slug from a document name.
 * "My Widget v2" → "my-widget-v2"
 */
export function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48) || "untitled";
}

/**
 * Record a redirect so old /view/<token> links can 308 to /@user/slug.
 */
export async function createShareRedirect(
  token: string,
  username: string,
  slug: string,
): Promise<void> {
  if (!isAuthEnabled()) return;
  const supabase = requireSupabase();
  const { error } = await supabase
    .from("share_redirects")
    .upsert({ token, username, slug });
  if (error) throw error;
}

/**
 * Look up a redirect for a share token. Returns null if none exists.
 * Used by the URL loader to 308 old /view/<token> links.
 */
export async function lookupShareRedirect(
  token: string,
): Promise<{ username: string; slug: string } | null> {
  const supabase = requireSupabase();
  const { data, error } = await supabase
    .from("share_redirects")
    .select("username, slug")
    .eq("token", token)
    .maybeSingle();
  if (error) return null;
  return data as { username: string; slug: string } | null;
}
