import { useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { Sparkle, Check } from "@phosphor-icons/react";
import { useTermsStore } from "@/stores/terms-store";

const ACCEPT_DELAY_MS = 220;

/**
 * Blocking overlay shown until the user accepts the Terms of Service and
 * acknowledges the Privacy Policy. Rendered as a Radix Dialog so it inherits
 * focus-trapping and screen-reader semantics; the dialog is non-dismissable
 * (Esc and click-outside are intercepted) — the only exit is accepting.
 *
 * Not rendered on the /privacy, /terms, /security legal pages themselves —
 * users must be able to read the documents before agreeing.
 */
export function TermsGate() {
  const accepted = useTermsStore((s) => s.accepted);
  const accept = useTermsStore((s) => s.accept);
  const [checked, setChecked] = useState(false);
  const [pressing, setPressing] = useState(false);

  if (accepted) return null;

  const onContinue = () => {
    if (!checked || pressing) return;
    setPressing(true);
    window.setTimeout(() => {
      accept();
    }, ACCEPT_DELAY_MS);
  };

  return (
    <Dialog.Root open modal>
      <Dialog.Portal>
        <Dialog.Overlay className="auth-overlay fixed inset-0 z-[100] bg-black/70 backdrop-blur-sm" />
        <Dialog.Content
          aria-labelledby="tos-gate-title"
          aria-describedby="tos-gate-desc"
          onEscapeKeyDown={(e) => e.preventDefault()}
          onPointerDownOutside={(e) => e.preventDefault()}
          onInteractOutside={(e) => e.preventDefault()}
          className="auth-content fixed left-1/2 top-1/2 z-[100] w-[440px] -translate-x-1/2 -translate-y-1/2 border border-border bg-card/85 backdrop-blur-xl shadow-[0_24px_60px_-12px_rgba(0,0,0,0.6)] focus:outline-none"
        >
          {/* Top brand edge highlight */}
          <div
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-0 h-px bg-[linear-gradient(90deg,transparent,var(--color-brand),transparent)] opacity-70"
          />

          <div className="flex flex-col items-center px-8 pt-8 pb-7">
            {/* Brand chip */}
            <div className="flex h-10 w-10 items-center justify-center border border-brand/30 bg-brand/12">
              <Sparkle size={22} weight="fill" className="text-brand" />
            </div>

            <Dialog.Title
              id="tos-gate-title"
              className="mt-4 text-[22px] font-semibold tracking-tight text-text"
            >
              Welcome to vcad
            </Dialog.Title>

            <Dialog.Description
              id="tos-gate-desc"
              className="mt-2 text-center text-[13px] leading-relaxed text-text-muted"
            >
              A quick housekeeping note before we hand you the kernel.
              <br />
              <span className="text-text-muted/70">
                We&rsquo;re open-source and don&rsquo;t sell your data — but
                the lawyers want their moment.
              </span>
            </Dialog.Description>

            {/* Custom checkbox row */}
            <label className="mt-6 flex w-full cursor-pointer items-start gap-3 text-[13px] text-text">
              <input
                type="checkbox"
                checked={checked}
                onChange={(e) => setChecked(e.target.checked)}
                className="peer sr-only"
              />
              <span
                aria-hidden
                data-checked={checked}
                className="mt-0.5 flex h-4 w-4 flex-shrink-0 items-center justify-center border border-border bg-bg/40 transition-colors duration-150 hover:border-text-muted/60 data-[checked=true]:border-brand data-[checked=true]:bg-brand peer-focus-visible:ring-2 peer-focus-visible:ring-brand/60 peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-card"
              >
                <Check
                  size={11}
                  weight="bold"
                  className="text-primary-foreground transition-[clip-path] duration-150"
                  style={{
                    clipPath: checked
                      ? "inset(0 0 0 0)"
                      : "inset(0 100% 0 0)",
                  }}
                />
              </span>
              <span className="leading-relaxed">
                I agree to the{" "}
                <a
                  href="/terms"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-brand underline decoration-brand/40 underline-offset-4 transition-colors hover:text-brand-hover hover:decoration-brand"
                >
                  Terms of Service
                </a>{" "}
                and acknowledge the{" "}
                <a
                  href="/privacy"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-brand underline decoration-brand/40 underline-offset-4 transition-colors hover:text-brand-hover hover:decoration-brand"
                >
                  Privacy Policy
                </a>
                .
              </span>
            </label>

            {/* CTA */}
            <button
              type="button"
              disabled={!checked || pressing}
              onClick={onContinue}
              className={
                checked
                  ? `upgrade-cta ${pressing ? "cta-press" : ""} mt-6 w-full border border-brand bg-brand px-4 py-3 text-[13px] font-medium text-primary-foreground transition-colors hover:bg-brand-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-brand/60 focus-visible:ring-offset-2 focus-visible:ring-offset-card`
                  : "mt-6 w-full cursor-not-allowed border border-border bg-bg/40 px-4 py-3 text-[13px] font-medium text-text-muted/50"
              }
            >
              Continue to vcad
            </button>

            <div className="mt-6 h-px w-full bg-border/70" />

            <p className="mt-3 text-[11px] text-text-muted/70">
              You can review these documents at any time from the account
              menu.
            </p>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
