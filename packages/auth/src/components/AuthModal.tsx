import { useState, type FormEvent } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import {
  X,
  EnvelopeSimple,
  GoogleLogo,
  GithubLogo,
} from "@phosphor-icons/react";
import { getAuthRedirectUrl, getSupabase, isTauriRuntime } from "../client";
import { signInWithOAuthPopup } from "../oauth-popup";
import type { GatedFeature } from "../hooks/useRequireAuth";

type OAuthProvider = "google" | "github";

interface AuthModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Feature that triggered the auth modal, for contextual messaging */
  feature?: GatedFeature;
}

const featureMessages: Record<GatedFeature, string> = {
  "cloud-sync": "to sync your designs",
  ai: "to use AI features",
  quotes: "to request quotes",
  "step-export": "to export STEP files",
  "version-history": "to access history",
};

/**
 * Modal dialog for user authentication.
 * Supports Google / GitHub OAuth and email magic link.
 */
export function AuthModal({ open, onOpenChange, feature }: AuthModalProps) {
  const [email, setEmail] = useState("");
  const [loading, setLoading] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const supabase = getSupabase();

  const signInWithProvider = async (provider: OAuthProvider) => {
    if (!supabase) return;

    setLoading(true);
    setError(null);

    window.dispatchEvent(
      new CustomEvent("vcad:sign-in-attempt", { detail: { provider } }),
    );

    const reportFailure = (message: string) => {
      setError(message);
      window.dispatchEvent(
        new CustomEvent("vcad:sign-in-attempt-failed", {
          detail: { provider, message },
        }),
      );
      setLoading(false);
    };

    if (isTauriRuntime()) {
      // Desktop: open the OS browser and let the deep-link bridge
      // complete the flow. Embedded webviews would be rejected by
      // Google/GitHub as "insecure".
      const { data, error } = await supabase.auth.signInWithOAuth({
        provider,
        options: {
          redirectTo: getAuthRedirectUrl(),
          skipBrowserRedirect: true,
        },
      });
      if (error) {
        reportFailure(error.message);
        return;
      }
      if (!data?.url) {
        reportFailure("OAuth provider did not return a redirect URL");
        return;
      }
      try {
        const { openUrl } = await import("@tauri-apps/plugin-opener");
        await openUrl(data.url);
        // Browser is launched; auth completes via the deep-link handler
        // and AuthProvider closes the modal. Re-enable the form so the
        // user can retry if they cancel in the browser.
        setLoading(false);
      } catch (err) {
        reportFailure(
          err instanceof Error ? err.message : "Failed to open browser",
        );
      }
      return;
    }

    // Web: drive the OAuth dance inside a popup so the user keeps
    // their place. `signInWithOAuthPopup` must run synchronously
    // enough that the click gesture still authorizes window.open —
    // it does (only state setters and the supabase call run before
    // the popup is opened).
    const result = await signInWithOAuthPopup(provider);

    if (result.ok) {
      // AuthProvider's onAuthStateChange handles the rest.
      setLoading(false);
      return;
    }

    if (result.cancelled) {
      // User closed the popup; quietly reset the form.
      setLoading(false);
      return;
    }

    if (result.popupBlocked) {
      // Fall back to today's full-page redirect so sign-in still
      // completes, even if the user loses ephemeral state.
      const { error: redirectError } = await supabase.auth.signInWithOAuth({
        provider,
        options: { redirectTo: getAuthRedirectUrl() },
      });
      if (redirectError) {
        reportFailure(redirectError.message);
        return;
      }
      // Supabase has navigated the page; leave loading set so the
      // form doesn't flash active mid-redirect.
      return;
    }

    reportFailure(result.error ?? "Sign-in failed");
  };

  const signInWithEmail = async (e: FormEvent) => {
    e.preventDefault();
    if (!supabase || !email) return;

    setLoading(true);
    setError(null);

    window.dispatchEvent(
      new CustomEvent("vcad:sign-in-attempt", { detail: { email } }),
    );

    const { error } = await supabase.auth.signInWithOtp({
      email,
      options: {
        emailRedirectTo: getAuthRedirectUrl(),
      },
    });

    if (error) {
      setError(error.message);
      window.dispatchEvent(
        new CustomEvent("vcad:sign-in-attempt-failed", {
          detail: { email, message: error.message },
        }),
      );
    } else {
      setSent(true);
      window.dispatchEvent(
        new CustomEvent("vcad:sign-in-attempt-sent", { detail: { email } }),
      );
    }
    setLoading(false);
  };

  const handleOpenChange = (open: boolean) => {
    onOpenChange(open);
    if (!open) {
      // Reset state when modal closes
      setTimeout(() => {
        setEmail("");
        setSent(false);
        setError(null);
        setLoading(false);
      }, 200);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={handleOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2 border border-border bg-card/95 backdrop-blur-sm shadow-lg focus:outline-none">
          {/* Close button */}
          <div className="absolute right-2 top-2 z-10">
            <Dialog.Close className="p-1 text-text-muted hover:bg-border/50 hover:text-text cursor-pointer">
              <X size={14} />
            </Dialog.Close>
          </div>

          {/* Content */}
          <div className="flex flex-col items-center px-6 py-5">
            {/* Header */}
            <Dialog.Title className="text-2xl font-bold tracking-tighter text-text mb-5">
              vcad<span className="text-brand">.</span>
            </Dialog.Title>

            {error && (
              <div className="w-full mb-4 p-2 bg-danger/10 border border-danger/30 text-xs text-danger text-center">
                {error}
              </div>
            )}

            {sent ? (
              <div className="text-center">
                <div className="w-10 h-10 mx-auto mb-3 flex items-center justify-center bg-brand/10">
                  <EnvelopeSimple size={20} className="text-brand" />
                </div>
                <p className="text-sm text-text mb-1">Check your email</p>
                <p className="text-xs text-text-muted mb-4">
                  Link sent to <span className="text-text">{email}</span>
                </p>
                <button
                  onClick={() => handleOpenChange(false)}
                  className="text-[10px] text-text-muted hover:text-text"
                >
                  close
                </button>
              </div>
            ) : (
              <div className="w-64 flex flex-col gap-2">
                <button
                  type="button"
                  onClick={() => signInWithProvider("google")}
                  disabled={loading}
                  className="w-full h-9 border border-border text-xs text-text hover:bg-border/50 disabled:opacity-30 disabled:cursor-not-allowed transition-colors flex items-center justify-center gap-2"
                >
                  <GoogleLogo size={14} weight="bold" />
                  Continue with Google
                </button>
                <button
                  type="button"
                  onClick={() => signInWithProvider("github")}
                  disabled={loading}
                  className="w-full h-9 border border-border text-xs text-text hover:bg-border/50 disabled:opacity-30 disabled:cursor-not-allowed transition-colors flex items-center justify-center gap-2"
                >
                  <GithubLogo size={14} weight="fill" />
                  Continue with GitHub
                </button>

                <div className="flex items-center gap-2 my-1">
                  <div className="flex-1 h-px bg-border" />
                  <span className="text-[10px] text-text-muted uppercase tracking-wider">or</span>
                  <div className="flex-1 h-px bg-border" />
                </div>

                <form onSubmit={signInWithEmail} className="flex flex-col gap-2">
                  <input
                    type="email"
                    placeholder="Email address"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    className="w-full h-9 px-3 bg-transparent border border-border text-xs text-text placeholder-text-muted/50 focus:outline-none focus:border-brand transition-colors"
                    disabled={loading}
                    autoComplete="email"
                  />
                  <button
                    type="submit"
                    disabled={loading || !email}
                    className="w-full h-9 border border-border text-xs text-text hover:bg-border/50 disabled:opacity-30 disabled:cursor-not-allowed transition-colors"
                  >
                    {loading ? "Sending..." : "Continue with email"}
                  </button>
                </form>
              </div>
            )}

            {/* Feature-context line (sits right above the legal footer) */}
            <p className="text-[10px] text-text-muted mt-4">
              {feature ? featureMessages[feature] : "sign in to save your work"}
            </p>
          </div>

          {/* Footer */}
          <div className="border-t border-border px-4 py-2.5 flex items-center justify-center">
            <p className="text-[10px] text-text-muted">
              <a href="https://vcad.io/terms" className="hover:text-text" target="_blank" rel="noopener noreferrer">terms</a>
              {" · "}
              <a href="https://vcad.io/privacy" className="hover:text-text" target="_blank" rel="noopener noreferrer">privacy</a>
            </p>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
