import { useEffect, type ReactNode } from "react";
import { ensureSession, getSupabase, isAuthEnabled } from "../client";
import { useAuthStore } from "../stores/auth-store";
import { useSignInDelightStore } from "../stores/sign-in-delight-store";

declare const posthog:
  | {
      identify: (id: string, properties?: Record<string, unknown>) => void;
      reset: () => void;
    }
  | undefined;

interface AuthProviderProps {
  children: ReactNode;
  /** Optional callback when user signs in */
  onSignIn?: () => void;
  /** Optional callback when user signs out */
  onSignOut?: () => void;
  /** Optional callback for first-time sign-in celebration */
  onFirstSignIn?: (firstName: string) => void;
}

/**
 * Check if this is a new user (created within the last hour).
 * Prevents confetti for existing users signing in on new devices.
 */
function isNewUser(createdAt: string | undefined): boolean {
  if (!createdAt) return false;
  const created = new Date(createdAt).getTime();
  const oneHourAgo = Date.now() - 60 * 60 * 1000;
  return created > oneHourAgo;
}

/**
 * Extract first name from user metadata or email.
 */
function getFirstName(user: { user_metadata?: { full_name?: string; name?: string }; email?: string }): string {
  const fullName = user.user_metadata?.full_name || user.user_metadata?.name;
  if (fullName) {
    return fullName.split(" ")[0] || "there";
  }
  // Fallback to email prefix
  if (user.email) {
    return user.email.split("@")[0] || "there";
  }
  return "there";
}

/**
 * Provider component that initializes authentication state.
 * Wrap your app with this component to enable auth features.
 *
 * @example
 * ```tsx
 * <AuthProvider onSignIn={() => triggerSync()}>
 *   <App />
 * </AuthProvider>
 * ```
 */
export function AuthProvider({
  children,
  onSignIn,
  onSignOut,
  onFirstSignIn,
}: AuthProviderProps) {
  const setSession = useAuthStore((s) => s.setSession);
  const setLoading = useAuthStore((s) => s.setLoading);
  const reset = useAuthStore((s) => s.reset);
  const hasSeenCelebration = useSignInDelightStore((s) => s.hasSeenSignInCelebration);
  const markCelebrationSeen = useSignInDelightStore((s) => s.markSignInCelebrationSeen);

  useEffect(() => {
    // If auth not configured, mark as initialized and return
    if (!isAuthEnabled()) {
      setLoading(false);
      reset();
      return;
    }

    const supabase = getSupabase();
    if (!supabase) {
      reset();
      return;
    }

    // Track the previous user id so we can detect anon→permanent upgrades
    // (Supabase emits USER_UPDATED with a new uid when an anonymous account
    // is linked to an OAuth identity).
    let previousUserId: string | null = null;
    let previousWasAnonymous = false;

    // Get initial session — and create an anonymous one if there isn't one,
    // so RLS predicates of the form `auth.uid() = user_id` work uniformly
    // for every user (anon or permanent).
    ensureSession().then((session) => {
      setSession(session);
      previousUserId = session?.user?.id ?? null;
      previousWasAnonymous = session?.user?.is_anonymous ?? false;

      if (session?.user && !session.user.is_anonymous) {
        if (typeof posthog !== "undefined") {
          posthog.identify(session.user.id, {
            email: session.user.email,
            auth_provider: session.user.app_metadata?.provider,
            created_at: session.user.created_at,
          });
        }
        onSignIn?.();
      }
    });

    // Listen for auth changes
    const {
      data: { subscription },
    } = supabase.auth.onAuthStateChange((event, session) => {
      setSession(session);

      // Detect anon → permanent upgrade. Supabase keeps the same uid when
      // an anon user links an identity (USER_UPDATED), but if for any
      // reason the uid changes we re-parent the chat threads so the user
      // doesn't lose their history at sign-in.
      const newUserId = session?.user?.id ?? null;
      const newIsAnon = session?.user?.is_anonymous ?? false;
      if (
        previousWasAnonymous &&
        !newIsAnon &&
        previousUserId &&
        newUserId &&
        previousUserId !== newUserId
      ) {
        const fromId = previousUserId;
        const toId = newUserId;
        void supabase.rpc("migrate_chat_threads_to_user", {
          from_user_id: fromId,
          to_user_id: toId,
        }).then(({ error }) => {
          if (error) {
            console.warn("[auth] migrate_chat_threads_to_user failed:", error.message);
          }
        });
      }
      previousUserId = newUserId;
      previousWasAnonymous = newIsAnon;

      if (event === "SIGNED_IN" && session?.user && !session.user.is_anonymous) {
        // Identify user in analytics
        if (typeof posthog !== "undefined") {
          posthog.identify(session.user.id, {
            email: session.user.email,
            auth_provider: session.user.app_metadata?.provider,
            created_at: session.user.created_at,
          });
        }

        const firstName = getFirstName(session.user);
        const provider = session.user.app_metadata?.provider;
        const isFirstTime = !hasSeenCelebration && isNewUser(session.user.created_at);

        // Check if this is a first-time sign-in celebration. First-time users
        // get the welcome toast instead of the generic "Signed in" toast.
        if (isFirstTime) {
          markCelebrationSeen();
          // Dispatch celebration event for CelebrationOverlay
          window.dispatchEvent(new CustomEvent("vcad:celebrate-sign-in"));
          // Dispatch welcome event with user's name for toast
          window.dispatchEvent(
            new CustomEvent("vcad:welcome-sign-in", { detail: { firstName } })
          );
          onFirstSignIn?.(firstName);
        } else {
          // Notify listeners that a returning user has signed in. The app
          // surfaces this as an in-app toast and (when backgrounded) an OS
          // notification so users know when an account sign-in happens.
          window.dispatchEvent(
            new CustomEvent("vcad:sign-in-success", {
              detail: {
                firstName,
                email: session.user.email,
                provider,
              },
            }),
          );
        }

        onSignIn?.();
      } else if (event === "SIGNED_OUT") {
        // Reset analytics identity
        if (typeof posthog !== "undefined") {
          posthog.reset();
        }
        window.dispatchEvent(new CustomEvent("vcad:sign-out"));
        onSignOut?.();
      }
    });

    return () => {
      subscription.unsubscribe();
    };
  }, [setSession, setLoading, reset, onSignIn, onSignOut, onFirstSignIn, hasSeenCelebration, markCelebrationSeen]);

  return <>{children}</>;
}
