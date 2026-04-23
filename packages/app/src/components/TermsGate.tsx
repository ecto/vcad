import { useEffect, useState } from "react";
import { useTermsStore } from "@/stores/terms-store";

/**
 * Blocking overlay shown until the user accepts the Terms of Service and
 * acknowledges the Privacy Policy. Rendered as a fixed full-viewport modal
 * on top of the app so the editor behind it is inert.
 *
 * Not rendered on the /privacy, /terms, /security legal pages themselves —
 * users must be able to read the documents before agreeing.
 */
export function TermsGate() {
  const accepted = useTermsStore((s) => s.accepted);
  const accept = useTermsStore((s) => s.accept);
  const [checked, setChecked] = useState(false);

  useEffect(() => {
    if (accepted) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prev;
    };
  }, [accepted]);

  if (accepted) return null;

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-labelledby="tos-gate-title"
    >
      <div className="w-full max-w-md bg-surface border border-border p-8 shadow-2xl">
        <h2
          id="tos-gate-title"
          className="text-xl font-semibold tracking-tight text-text"
        >
          Welcome to vcad
        </h2>
        <p className="mt-3 text-sm text-text-muted leading-relaxed">
          Before you start modeling, please review and accept our terms.
        </p>

        <label className="mt-6 flex items-start gap-3 text-sm text-text cursor-pointer">
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => setChecked(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            I agree to the{" "}
            <a
              href="/terms"
              target="_blank"
              rel="noopener noreferrer"
              className="text-brand hover:underline"
            >
              Terms of Service
            </a>{" "}
            and acknowledge the{" "}
            <a
              href="/privacy"
              target="_blank"
              rel="noopener noreferrer"
              className="text-brand hover:underline"
            >
              Privacy Policy
            </a>
            .
          </span>
        </label>

        <button
          type="button"
          disabled={!checked}
          onClick={accept}
          className="mt-6 w-full bg-brand px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
        >
          Continue to vcad
        </button>

        <p className="mt-4 text-[11px] text-text-muted/70">
          You can review these documents at any time from the account menu.
        </p>
      </div>
    </div>
  );
}
