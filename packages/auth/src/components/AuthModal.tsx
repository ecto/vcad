import { useEffect, useRef, useState, type FormEvent } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import {
  X,
  EnvelopeSimple,
  ArrowRight,
  CircleNotch,
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

const RESEND_COOLDOWN_SECS = 30;

const isValidEmail = (s: string) => /\S+@\S+\.\S+/.test(s.trim());

/**
 * Modal dialog for user authentication.
 * Supports Google / GitHub OAuth and email magic link.
 */
export function AuthModal({ open, onOpenChange, feature }: AuthModalProps) {
  const [email, setEmail] = useState("");
  const [loading, setLoading] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [resendIn, setResendIn] = useState(0);
  const emailRef = useRef<HTMLInputElement>(null);

  const supabase = getSupabase();

  useEffect(() => {
    if (resendIn <= 0) return;
    const id = window.setTimeout(() => setResendIn((s) => s - 1), 1000);
    return () => window.clearTimeout(id);
  }, [resendIn]);

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

  const sendMagicLink = async () => {
    if (!supabase || !isValidEmail(email)) return;

    setLoading(true);
    setError(null);

    window.dispatchEvent(
      new CustomEvent("vcad:sign-in-attempt", { detail: { email } }),
    );

    const { error } = await supabase.auth.signInWithOtp({
      email,
      options: { emailRedirectTo: getAuthRedirectUrl() },
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
      setResendIn(RESEND_COOLDOWN_SECS);
      window.dispatchEvent(
        new CustomEvent("vcad:sign-in-attempt-sent", { detail: { email } }),
      );
    }
    setLoading(false);
  };

  const onEmailSubmit = (e: FormEvent) => {
    e.preventDefault();
    sendMagicLink();
  };

  const handleOpenChange = (open: boolean) => {
    onOpenChange(open);
    if (!open) {
      setTimeout(() => {
        setEmail("");
        setSent(false);
        setError(null);
        setLoading(false);
        setResendIn(0);
      }, 200);
    }
  };

  const emailValid = isValidEmail(email);
  const subtitle = feature
    ? featureMessages[feature]
    : "save your work, fork files, and use AI";

  return (
    <Dialog.Root open={open} onOpenChange={handleOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="auth-overlay fixed inset-0 z-50 bg-black/60" />
        <Dialog.Content
          aria-describedby={undefined}
          onOpenAutoFocus={(e) => {
            // Radix focuses the first focusable (the close button) by default.
            // Prefer the email field — it's the only input on the form.
            e.preventDefault();
            emailRef.current?.focus();
          }}
          className="auth-content fixed left-1/2 top-1/2 z-50 w-[360px] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-xl border border-border bg-surface shadow-xl focus:outline-none"
        >
          {/* Close button */}
          <Dialog.Close
            aria-label="Close"
            className="absolute right-3 top-3 z-10 flex h-7 w-7 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-hover hover:text-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
          >
            <X size={16} />
          </Dialog.Close>

          {/* Content */}
          <div className="flex flex-col items-center px-8 pt-10 pb-7">
            {/* Header */}
            <Dialog.Title className="text-5xl font-bold tracking-tighter leading-none text-text select-none">
              vcad<span className="text-brand">.</span>
            </Dialog.Title>
            <p className="mt-3 text-[13px] text-text-muted text-center">
              {subtitle}
            </p>

            {error && (
              <div className="mt-5 w-full rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-center text-xs text-danger">
                {error}
              </div>
            )}

            {sent ? (
              <SentState
                email={email}
                resendIn={resendIn}
                loading={loading}
                onResend={sendMagicLink}
                onClose={() => handleOpenChange(false)}
              />
            ) : (
              <div className="mt-6 flex w-full flex-col gap-2">
                <ProviderButton
                  onClick={() => signInWithProvider("google")}
                  disabled={loading}
                  icon={<GoogleColorLogo />}
                  label="Continue with Google"
                />
                <ProviderButton
                  onClick={() => signInWithProvider("github")}
                  disabled={loading}
                  icon={<GithubLogoMark />}
                  label="Continue with GitHub"
                />

                <div className="my-2 flex items-center gap-3">
                  <div className="h-px flex-1 bg-border" />
                  <span className="text-[11px] text-text-muted">or</span>
                  <div className="h-px flex-1 bg-border" />
                </div>

                <form
                  onSubmit={onEmailSubmit}
                  className="flex flex-col gap-2"
                  noValidate
                >
                  <input
                    ref={emailRef}
                    type="email"
                    placeholder="you@example.com"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    className="h-10 w-full rounded-md border border-border bg-bg px-3 text-[13px] text-text placeholder-text-muted transition-colors focus:border-brand focus:outline-none focus:ring-2 focus:ring-brand/30"
                    disabled={loading}
                    autoComplete="email"
                  />
                  <EmailCta
                    loading={loading}
                    active={emailValid && !loading}
                  />
                </form>
              </div>
            )}
          </div>

          {/* Footer */}
          <div className="flex items-center justify-center border-t border-border-soft px-4 py-3">
            <p className="text-[11px] text-text-muted">
              <a
                href="https://vcad.io/terms"
                className="hover:text-text transition-colors"
                target="_blank"
                rel="noopener noreferrer"
              >
                Terms
              </a>
              <span className="mx-2 text-text-tert">·</span>
              <a
                href="https://vcad.io/privacy"
                className="hover:text-text transition-colors"
                target="_blank"
                rel="noopener noreferrer"
              >
                Privacy
              </a>
              <span className="mx-2 text-text-tert">·</span>
              <a
                href="https://vcad.io/security"
                className="hover:text-text transition-colors"
                target="_blank"
                rel="noopener noreferrer"
              >
                Security
              </a>
            </p>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ProviderButton({
  onClick,
  disabled,
  icon,
  label,
}: {
  onClick: () => void;
  disabled: boolean;
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="flex h-10 w-full items-center justify-center gap-2.5 rounded-md border border-border bg-transparent text-[13px] font-medium text-text transition-colors duration-150 hover:bg-hover disabled:cursor-not-allowed disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand"
    >
      {icon}
      {label}
    </button>
  );
}

function EmailCta({ loading, active }: { loading: boolean; active: boolean }) {
  if (loading) {
    return (
      <button
        type="submit"
        disabled
        className="flex h-10 w-full items-center justify-center gap-2 rounded-md border border-border bg-transparent text-[13px] text-text-muted"
      >
        <CircleNotch size={14} className="animate-spin" />
        Sending…
      </button>
    );
  }

  if (active) {
    return (
      <button
        type="submit"
        className="group flex h-10 w-full items-center justify-center gap-2 rounded-md border border-brand bg-brand text-[13px] font-medium text-primary-foreground transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand focus-visible:ring-offset-2 focus-visible:ring-offset-surface"
      >
        <span>Continue with email</span>
        <ArrowRight
          size={14}
          weight="bold"
          className="transition-transform duration-150 group-hover:translate-x-0.5"
        />
      </button>
    );
  }

  return (
    <button
      type="submit"
      disabled
      className="flex h-10 w-full items-center justify-center gap-2 rounded-md border border-border bg-transparent text-[13px] text-text-muted/60"
    >
      Continue with email
    </button>
  );
}

function SentState({
  email,
  resendIn,
  loading,
  onResend,
  onClose,
}: {
  email: string;
  resendIn: number;
  loading: boolean;
  onResend: () => void;
  onClose: () => void;
}) {
  return (
    <div className="auth-success-in mt-6 flex w-full flex-col items-center text-center">
      <div className="mb-4 flex h-12 w-12 items-center justify-center rounded-full border border-brand/30 bg-brand/10">
        <EnvelopeSimple size={22} className="text-brand" />
      </div>
      <p className="text-[15px] font-medium text-text">Check your email</p>
      <p className="mt-1.5 text-[12px] text-text-muted">
        We sent a sign-in link to{" "}
        <span className="text-text">{email}</span>
      </p>

      <button
        type="button"
        onClick={onResend}
        disabled={resendIn > 0 || loading}
        className="mt-5 text-[12px] text-text-muted underline-offset-4 transition-colors hover:text-text hover:underline disabled:cursor-not-allowed disabled:opacity-50 disabled:no-underline"
      >
        {resendIn > 0 ? `Resend link in ${resendIn}s` : "Resend link"}
      </button>

      <button
        type="button"
        onClick={onClose}
        className="mt-2 text-[11px] text-text-muted/70 transition-colors hover:text-text"
      >
        Close
      </button>
    </div>
  );
}

function GoogleColorLogo() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 48 48"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
    >
      <path
        fill="#FFC107"
        d="M43.611 20.083H42V20H24v8h11.303c-1.649 4.657-6.08 8-11.303 8-6.627 0-12-5.373-12-12s5.373-12 12-12c3.059 0 5.842 1.154 7.961 3.039l5.657-5.657C34.046 6.053 29.268 4 24 4 12.955 4 4 12.955 4 24s8.955 20 20 20 20-8.955 20-20c0-1.341-.138-2.65-.389-3.917z"
      />
      <path
        fill="#FF3D00"
        d="M6.306 14.691l6.571 4.819C14.655 15.108 18.961 12 24 12c3.059 0 5.842 1.154 7.961 3.039l5.657-5.657C34.046 6.053 29.268 4 24 4 16.318 4 9.656 8.337 6.306 14.691z"
      />
      <path
        fill="#4CAF50"
        d="M24 44c5.166 0 9.86-1.977 13.409-5.192l-6.19-5.238C29.211 35.091 26.715 36 24 36c-5.202 0-9.619-3.317-11.283-7.946l-6.522 5.025C9.505 39.556 16.227 44 24 44z"
      />
      <path
        fill="#1976D2"
        d="M43.611 20.083H42V20H24v8h11.303c-.792 2.237-2.231 4.166-4.087 5.571.001-.001.002-.001.003-.002l6.19 5.238C36.971 39.205 44 34 44 24c0-1.341-.138-2.65-.389-3.917z"
      />
    </svg>
  );
}

function GithubLogoMark() {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      className="text-text"
    >
      <path
        fill="currentColor"
        d="M12 .5C5.73.5.66 5.58.66 11.86c0 5.02 3.25 9.27 7.76 10.77.57.1.78-.25.78-.55 0-.27-.01-1-.02-1.95-3.16.69-3.83-1.52-3.83-1.52-.52-1.32-1.27-1.67-1.27-1.67-1.04-.71.08-.7.08-.7 1.14.08 1.74 1.17 1.74 1.17 1.02 1.75 2.68 1.25 3.33.95.1-.74.4-1.25.72-1.54-2.52-.29-5.18-1.26-5.18-5.62 0-1.24.44-2.26 1.17-3.05-.12-.29-.51-1.45.11-3.03 0 0 .96-.31 3.14 1.16.91-.25 1.89-.38 2.86-.39.97 0 1.95.13 2.86.39 2.18-1.47 3.14-1.16 3.14-1.16.62 1.58.23 2.74.11 3.03.73.79 1.17 1.81 1.17 3.05 0 4.37-2.67 5.33-5.21 5.61.41.35.78 1.04.78 2.1 0 1.51-.01 2.73-.01 3.1 0 .3.21.66.79.55 4.5-1.51 7.75-5.76 7.75-10.77C23.34 5.58 18.27.5 12 .5z"
      />
    </svg>
  );
}
