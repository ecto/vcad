import { useCallback, useEffect, useState } from "react";
import { getSupabase } from "../client";
import { useAuthStore } from "../stores/auth-store";

export interface UserPreferences {
  /** When true, chat conversations are stored for SFT / training. Default true. */
  share_chat_conversations: boolean;
}

const DEFAULT_PREFS: UserPreferences = {
  share_chat_conversations: true,
};

/**
 * Load + update the signed-in user's preferences row. For anonymous users
 * returns the defaults with no-op setters.
 *
 * Creates the row on first update via upsert.
 */
export function useUserPreferences() {
  const user = useAuthStore((s) => s.user);
  const [prefs, setPrefs] = useState<UserPreferences>(DEFAULT_PREFS);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Load on mount / user change
  useEffect(() => {
    if (!user) {
      setPrefs(DEFAULT_PREFS);
      return;
    }
    const supabase = getSupabase();
    if (!supabase) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    supabase
      .from("user_preferences")
      .select("share_chat_conversations")
      .eq("user_id", user.id)
      .maybeSingle()
      .then(({ data, error: err }) => {
        if (cancelled) return;
        if (err) {
          setError(err.message);
          setPrefs(DEFAULT_PREFS);
        } else if (data) {
          setPrefs({ share_chat_conversations: data.share_chat_conversations });
        } else {
          setPrefs(DEFAULT_PREFS);
        }
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [user]);

  const updatePreferences = useCallback(
    async (updates: Partial<UserPreferences>) => {
      if (!user) return;
      const supabase = getSupabase();
      if (!supabase) return;
      // Optimistic update
      setPrefs((prev) => ({ ...prev, ...updates }));
      const { error: err } = await supabase
        .from("user_preferences")
        .upsert(
          {
            user_id: user.id,
            ...prefs,
            ...updates,
          },
          { onConflict: "user_id" },
        );
      if (err) {
        setError(err.message);
      }
    },
    [user, prefs],
  );

  return { preferences: prefs, loading, error, updatePreferences };
}
