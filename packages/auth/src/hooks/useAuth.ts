import { useAuthStore } from "../stores/auth-store";

/**
 * Hook to access current auth state.
 * Returns user, session, loading state, and helper methods.
 */
export function useAuth() {
  const user = useAuthStore((s) => s.user);
  const session = useAuthStore((s) => s.session);
  const loading = useAuthStore((s) => s.loading);
  const initialized = useAuthStore((s) => s.initialized);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);

  // A user is "permanently signed in" if they have a session AND that session
  // isn't a Supabase anonymous one. Anonymous users have an auth.uid() (so RLS
  // works) but the UI still treats them as not-signed-in for everything that
  // matters: auth modals, sync banners, paid entitlements, sign-out menus.
  const hasPermanentIdentity = !!user && !isAnonymous;

  return {
    /** Current authenticated user or null */
    user,
    /** Current session with access token */
    session,
    /** True while checking initial session */
    loading,
    /** True after initial session check completes */
    initialized,
    /** True if user is signed in with a permanent (non-anon) identity */
    isAuthenticated: hasPermanentIdentity,
    /** True if session check is complete and user has no permanent identity. */
    isAnonymous: initialized && !hasPermanentIdentity,
  };
}
