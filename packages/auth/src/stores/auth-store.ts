import { create } from "zustand";
import type { User, Session } from "@supabase/supabase-js";

interface AuthState {
  /** Current authenticated user, or null if not signed in */
  user: User | null;
  /** Current session with access token */
  session: Session | null;
  /** True while checking initial session state */
  loading: boolean;
  /** True if initial session check is complete */
  initialized: boolean;
  /** True when the user is signed in via supabase.auth.signInAnonymously().
   * Anonymous users have a real auth.uid() so RLS works, but they haven't
   * linked an identity yet — UI should still treat them as "not signed in"
   * for things like the auth modal, sync banners, etc. */
  isAnonymous: boolean;

  // Actions
  setSession: (session: Session | null) => void;
  setLoading: (loading: boolean) => void;
  reset: () => void;
}

export const useAuthStore = create<AuthState>((set) => ({
  user: null,
  session: null,
  loading: true,
  initialized: false,
  isAnonymous: false,

  setSession: (session) =>
    set({
      session,
      user: session?.user ?? null,
      isAnonymous: session?.user?.is_anonymous ?? false,
      loading: false,
      initialized: true,
    }),

  setLoading: (loading) => set({ loading }),

  reset: () =>
    set({
      user: null,
      session: null,
      isAnonymous: false,
      loading: false,
      initialized: true,
    }),
}));
