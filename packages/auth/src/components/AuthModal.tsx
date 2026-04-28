import { useEffect, useState, type FormEvent } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import {
  X,
  EnvelopeSimple,
  GoogleLogo,
  GithubLogo,
  ArrowRight,
  CircleNotch,
} from "@phosphor-icons/react";
import { getAuthRedirectUrl, getSupabase, isTauriRuntime } from "../client";
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

    const tauri = isTauriRuntime();

    const { data, error } = await supabase.auth.signInWithOAuth({
      provider,
      options: {
        redirectTo: getAuthRedirectUrl(),
        skipBrowserRedirect: tauri,
      },
    });

    const reportFailure = (message: string) => {
      setError(message);
      window.dispatchEvent(
        new CustomEvent("vcad:sign-in-attempt-failed", {
          detail: { provider, message },
        }),
      );
      setLoading(false);
    };

    if (error) {
      reportFailure(error.message);
      return;
    }

    if (tauri) {
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
        <Dialog.Overlay className="auth-overlay fixed inset-0 z-50 bg-black/65 backdrop-blur-sm" />
        <Dialog.Content
          aria-describedby={undefined}
          className="auth-content fixed left-1/2 top-1/2 z-50 w-[340px] -translate-x-1/2 -translate-y-1/2 border border-border bg-card/85 backdrop-blur-xl shadow-[0_24px_60px_-12px_rgba(0,0,0,0.6)] focus:outline-none"
        >
          {/* Top brand edge highlight */}
          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-0 h-px bg-[linear-gradient(90deg,transparent,var(--color-brand),transparent)] opacity-70"
          />

          {/* Close button */}
          <Dialog.Close
            aria-label="Close"
            className="absolute right-2.5 top-2.5 z-10 p-1 text-text-muted/70 transition-colors hover:bg-hover hover:text-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 focus-visible:ring-offset-2 focus-visible:ring-offset-card"
          >
            <X size={12} />
          </Dialog.Close>

          {/* Content */}
          <div className="flex flex-col items-center px-7 pt-7 pb-6">
            {/* Header */}
            <Dialog.Title className="text-3xl font-bold tracking-tighter text-text">
              vcad
              <span
                className="text-brand"
                style={{
                  display: "inline-block",
                  animation: "vcad-pulse 2.4s ease-in-out infinite",
                }}
              >
                .
              </span>
            </Dialog.Title>
            <p className="mt-1 text-[11px] text-text-muted/80">
              {subtitle}
            </p>

            {error && (
              <div className="mt-4 w-full border border-danger/30 bg-danger/10 p-2 text-center text-xs text-danger">
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
              <div className="mt-5 flex w-full flex-col gap-2">
                <ProviderButton
                  onClick={() => signInWithProvider("google")}
                  disabled={loading}
                  icon={<GoogleLogo size={16} weight="bold" />}
                  label="Continue with Google"
                />
                <ProviderButton
                  onClick={() => signInWithProvider("github")}
                  disabled={loading}
                  icon={<GithubLogo size={16} weight="fill" />}
                  label="Continue with GitHub"
                />

                <div className="my-3 flex items-center gap-2">
                  <div className="h-px flex-1 bg-border/70" />
                  <span className="text-[9.5px] uppercase tracking-[0.18em] text-text-muted">
                    or
                  </span>
                  <div className="h-px flex-1 bg-border/70" />
                </div>

                <form
                  onSubmit={onEmailSubmit}
                  className="flex flex-col gap-2"
                  noValidate
                >
                  <input
                    type="email"
                    placeholder="Email address"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    className="h-11 w-full border border-border bg-bg/60 px-3 text-[13px] text-text placeholder-text-muted/50 transition-colors focus:border-brand focus:bg-bg/80 focus:outline-none"
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
          <div className="flex items-center justify-center border-t border-border px-4 py-2.5">
            <p className="text-[10px] text-text-muted">
              <a
                href="https://vcad.io/terms"
                className="hover:text-text"
                target="_blank"
                rel="noopener noreferrer"
              >
                terms
              </a>
              {" · "}
              <a
                href="https://vcad.io/privacy"
                className="hover:text-text"
                target="_blank"
                rel="noopener noreferrer"
              >
                privacy
              </a>
              {" · "}
              <a
                href="https://vcad.io/security"
                className="hover:text-text"
                target="_blank"
                rel="noopener noreferrer"
              >
                security
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
      className="flex h-11 w-full items-center justify-center gap-2.5 border border-border text-[13px] text-text transition-colors duration-150 hover:border-text-muted/40 hover:bg-hover disabled:cursor-not-allowed disabled:opacity-30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 focus-visible:ring-offset-2 focus-visible:ring-offset-card"
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
        className="flex h-11 w-full items-center justify-center gap-2 border border-border bg-transparent text-[13px] text-text-muted/70"
      >
        <CircleNotch size={12} className="animate-spin" />
        Sending…
      </button>
    );
  }

  if (active) {
    return (
      <button
        type="submit"
        className="upgrade-cta group flex h-11 w-full items-center justify-center gap-2 border border-brand bg-brand text-[13px] font-medium text-primary-foreground transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 focus-visible:ring-offset-2 focus-visible:ring-offset-card"
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
      className="flex h-11 w-full items-center justify-center gap-2 border border-border bg-transparent text-[13px] text-text-muted/60"
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
    <div className="auth-success-in mt-5 flex w-full flex-col items-center text-center">
      <div className="mb-3 flex h-11 w-11 items-center justify-center border border-brand/30 bg-brand/12">
        <EnvelopeSimple size={22} className="text-brand" />
      </div>
      <p className="text-sm font-medium text-text">Check your email</p>
      <p className="mt-1 text-[11px] text-text-muted">
        We sent a sign-in link to{" "}
        <span className="text-text">{email}</span>
      </p>

      <button
        type="button"
        onClick={onResend}
        disabled={resendIn > 0 || loading}
        className="mt-4 text-[11px] text-text-muted underline-offset-4 transition-colors hover:text-text hover:underline disabled:cursor-not-allowed disabled:opacity-50 disabled:no-underline"
      >
        {resendIn > 0 ? `Resend link in ${resendIn}s` : "Resend link"}
      </button>

      <button
        type="button"
        onClick={onClose}
        className="mt-1 text-[10px] text-text-muted/70 hover:text-text"
      >
        close
      </button>
    </div>
  );
}
