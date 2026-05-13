import { useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { FileArrowUp } from "@phosphor-icons/react/dist/ssr/FileArrowUp";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { GitBranch } from "@phosphor-icons/react/dist/ssr/GitBranch";
import { Lightning } from "@phosphor-icons/react/dist/ssr/Lightning";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { ArrowRight } from "@phosphor-icons/react/dist/ssr/ArrowRight";
import { GithubLogo } from "@phosphor-icons/react/dist/ssr/GithubLogo";
import { Book } from "@phosphor-icons/react/dist/ssr/Book";
import { ChatCircle } from "@phosphor-icons/react/dist/ssr/ChatCircle";
import {
  TIERS,
  PURCHASABLE_TIERS,
  formatTokens,
  useBillingStore,
  type PaidTierId,
} from "@vcad/core";
import { cn } from "@/lib/utils";
import { startCheckout } from "@/lib/billing-api";

// ---------------------------------------------------------------------------
// Feature grid — the product pitch. Each card is one "why vcad" reason.
// ---------------------------------------------------------------------------

const FEATURES = [
  {
    icon: Cube,
    title: "Real BRep kernel",
    desc: "Solid modeling with half-edge topology, NURBS, and exact predicates — not mesh hacks.",
    color: "text-emerald-400",
  },
  {
    icon: Sparkle,
    title: "AI-native",
    desc: "Describe what you want in words. The assistant creates, modifies, and explains geometry.",
    color: "text-brand",
  },
  {
    icon: FileArrowUp,
    title: "STEP in, STL out",
    desc: "Drag-drop STEP import. Export to STL, GLB, STEP, DXF. Interops with Fusion, SolidWorks, etc.",
    color: "text-amber-400",
  },
  {
    icon: Globe,
    title: "Runs in your browser",
    desc: "No install, no GPU requirement. WebAssembly kernel + WebGL viewport. Works on any device.",
    color: "text-blue-400",
  },
  {
    icon: GitBranch,
    title: "Open source",
    desc: "Your data, your terms. Self-host or use vcad.io. MIT-licensed kernel, Apache-2 app.",
    color: "text-violet-400",
  },
  {
    icon: Lightning,
    title: "Physics & simulation",
    desc: "phyz articulated rigid-body physics, joint simulation, and gym-style RL interface for robotics.",
    color: "text-orange-400",
  },
] as const;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function ProductModal({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const snapshot = useBillingStore((s) => s.snapshot);
  const currentTier = snapshot?.tier ?? "free";
  const [busy, setBusy] = useState<PaidTierId | null>(null);

  const handleCheckout = async (tier: PaidTierId) => {
    setBusy(tier);
    try {
      const url = await startCheckout(tier);
      const a = document.createElement("a");
      a.href = url;
      a.rel = "noopener";
      a.click();
    } catch (err) {
      console.error("[product] checkout error:", err);
      setBusy(null);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          data-tauri-drag-region=""
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-lg -translate-x-1/2 -translate-y-1/2",
            "rounded-xl border border-border bg-surface shadow-xl select-none",
            "focus:outline-none",
            "max-h-[90vh] overflow-y-auto scrollbar-thin",
          )}
        >
          <Dialog.Close className="sticky top-0 float-right z-10 m-3 p-1.5 text-text-muted hover:text-text hover:bg-hover transition-colors cursor-pointer">
            <X size={14} />
          </Dialog.Close>

          <div className="p-8 pt-6">
            {/* Hero */}
            <div className="text-center mb-6">
              <Dialog.Title className="text-3xl font-bold tracking-tighter text-text mb-1">
                vcad<span className="text-brand">.</span>
              </Dialog.Title>
              <Dialog.Description className="text-xs text-text-muted">
                Open-source parametric CAD, in your browser
              </Dialog.Description>
            </div>

            {/* Feature grid */}
            <div className="grid grid-cols-2 gap-2 mb-8">
              {FEATURES.map((f) => (
                <div
                  key={f.title}
                  className="flex flex-col gap-1 p-3 bg-bg"
                >
                  <f.icon size={16} className={f.color} />
                  <span className="text-[11px] font-semibold text-text">
                    {f.title}
                  </span>
                  <span className="text-[10px] leading-snug text-text-muted">
                    {f.desc}
                  </span>
                </div>
              ))}
            </div>

            {/* Pricing section */}
            <div className="mb-8">
              <div className="flex items-center gap-3 mb-4">
                <div className="h-px flex-1 bg-border" />
                <span className="text-[9px] font-semibold uppercase tracking-[0.14em] text-text-muted">
                  Unlock more with Pro
                </span>
                <div className="h-px flex-1 bg-border" />
              </div>

              <div className="grid grid-cols-2 gap-2">
                {PURCHASABLE_TIERS.map((tierId) => {
                  const tier = TIERS[tierId];
                  const isCurrent = currentTier === tierId;
                  const isRecommended =
                    currentTier === "free" && tierId === "pro";
                  return (
                    <div
                      key={tierId}
                      className={cn(
                        "relative flex flex-col border p-4",
                        isRecommended
                          ? "border-brand/60 ring-1 ring-brand/20 bg-brand/[0.04]"
                          : "border-border bg-bg",
                      )}
                    >
                      {isRecommended && (
                        <div className="absolute -top-[9px] left-3 bg-brand px-1.5 py-0.5 text-[8px] font-bold uppercase tracking-[0.14em] text-white">
                          Popular
                        </div>
                      )}
                      <div className="flex items-baseline justify-between mb-1">
                        <span className="text-[13px] font-bold tracking-tight text-text">
                          {tier.name}
                        </span>
                        {isCurrent && (
                          <span className="bg-brand/15 px-1 py-0.5 text-[7px] font-bold uppercase tracking-wider text-brand">
                            Current
                          </span>
                        )}
                      </div>
                      <div className="flex items-baseline gap-1 mb-2">
                        <span className="text-lg font-bold tracking-tighter text-text">
                          ${tier.priceMonthlyUsd}
                        </span>
                        <span className="text-[9px] text-text-muted">/mo</span>
                      </div>
                      <ul className="flex-1 space-y-1 mb-3">
                        {tier.perks.map((perk) => (
                          <li
                            key={perk}
                            className="flex items-start gap-1 text-[10px] text-text-muted"
                          >
                            <Check
                              size={9}
                              weight="bold"
                              className="mt-[3px] shrink-0 text-brand"
                            />
                            <span>{perk}</span>
                          </li>
                        ))}
                      </ul>
                      <button
                        type="button"
                        disabled={busy !== null || isCurrent}
                        onClick={() => handleCheckout(tierId)}
                        className={cn(
                          "group flex h-8 items-center justify-center gap-1.5",
                          "text-[10px] font-bold uppercase tracking-[0.1em]",
                          "transition-[background-color,transform,box-shadow] duration-150",
                          "disabled:cursor-not-allowed disabled:opacity-40",
                          "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-brand",
                          isCurrent
                            ? "border border-border text-text-muted"
                            : "bg-brand text-white hover:bg-brand-hover hover:shadow-[0_6px_20px_-6px_rgba(249,38,114,0.5)] active:translate-y-[1px]",
                        )}
                      >
                        {busy === tierId ? (
                          "Redirecting..."
                        ) : isCurrent ? (
                          "Current plan"
                        ) : (
                          <>
                            Get {tier.name}
                            <ArrowRight
                              size={10}
                              weight="bold"
                              className="transition-transform group-hover:translate-x-0.5"
                            />
                          </>
                        )}
                      </button>
                    </div>
                  );
                })}
              </div>

              <p className="mt-3 text-center text-[9px] text-text-muted">
                Free tier includes {formatTokens(TIERS.free.monthlyTokenLimit)} chat
                tokens/mo · All CAD tools are always free · Cancel anytime
              </p>
            </div>

            {/* Footer links */}
            <div className="flex items-center justify-center gap-6 text-xs">
              <a
                href="https://github.com/nicholaschuayunzhi/vcad"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1.5 text-text-muted hover:text-text transition-colors"
              >
                <GithubLogo size={13} />
                <span>GitHub</span>
              </a>
              <a
                href="https://vcad.io/docs"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1.5 text-text-muted hover:text-text transition-colors"
              >
                <Book size={13} />
                <span>Docs</span>
              </a>
              <a
                href="https://discord.gg/vcad"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-1.5 text-text-muted hover:text-text transition-colors"
              >
                <ChatCircle size={13} />
                <span>Discord</span>
              </a>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
