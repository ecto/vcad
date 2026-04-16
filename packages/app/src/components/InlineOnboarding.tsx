import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { Wrench } from "@phosphor-icons/react/dist/ssr/Wrench";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { GearSix } from "@phosphor-icons/react/dist/ssr/GearSix";
import { FlowerLotus } from "@phosphor-icons/react/dist/ssr/FlowerLotus";
import { Robot } from "@phosphor-icons/react/dist/ssr/Robot";
import { Wine } from "@phosphor-icons/react/dist/ssr/Wine";
import { Smiley } from "@phosphor-icons/react/dist/ssr/Smiley";
import { Square } from "@phosphor-icons/react/dist/ssr/Square";
import { Waveform } from "@phosphor-icons/react/dist/ssr/Waveform";
import { Star } from "@phosphor-icons/react/dist/ssr/Star";
import type { Icon } from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import { useDocumentStore, useUiStore, useChatStore, parseVcadFile } from "@vcad/core";
import { useAuth, isAuthEnabled, AuthModal } from "@vcad/auth";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { examples } from "@/data/examples";
import type { Example } from "@/data/examples";

interface InlineOnboardingProps {
  visible: boolean;
}

/** Icon + accent color per example. Falls back to a cube if unknown. */
const EXAMPLE_ICONS: Record<string, { Icon: Icon; color: string }> = {
  plate: { Icon: Square, color: "text-[#94A3B8]" },
  bracket: { Icon: Wrench, color: "text-[#60A5FA]" },
  mascot: { Icon: Smiley, color: "text-[#FBBF24]" },
  container: { Icon: Package, color: "text-[#A78BFA]" },
  flange: { Icon: GearSix, color: "text-[#FB7185]" },
  ribbon: { Icon: Star, color: "text-[#F472B6]" },
  spring: { Icon: Waveform, color: "text-[#22D3EE]" },
  vase: { Icon: FlowerLotus, color: "text-[#34D399]" },
  wineglass: { Icon: Wine, color: "text-[#E879F9]" },
  "robot-arm": { Icon: Robot, color: "text-[#FB923C]" },
};

/** Six highlighted examples — the rest appear below in a smaller strip. */
const HERO_EXAMPLE_IDS = ["bracket", "container", "robot-arm", "vase", "flange", "spring"];

export function InlineOnboarding({ visible }: InlineOnboardingProps) {
  const [dontShowAgain, setDontShowAgain] = useState(false);
  const dismissWelcomeModal = useOnboardingStore((s) => s.dismissWelcomeModal);
  const hideWelcomeThisSession = useOnboardingStore(
    (s) => s.hideWelcomeThisSession,
  );
  const fileInputRef = useRef<HTMLInputElement>(null);

  const loadDocument = useDocumentStore((s) => s.loadDocument);
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const select = useUiStore((s) => s.select);
  const setTransformMode = useUiStore((s) => s.setTransformMode);
  const setChatOpen = useChatStore((s) => s.setOpen);
  const incrementProjectsCreated = useOnboardingStore(
    (s) => s.incrementProjectsCreated,
  );

  const { user, isAuthenticated } = useAuth();
  const authEnabled = isAuthEnabled();
  const [showAuthModal, setShowAuthModal] = useState(false);

  const hide = useCallback(() => {
    if (dontShowAgain) {
      dismissWelcomeModal();
    } else {
      hideWelcomeThisSession();
    }
  }, [dontShowAgain, dismissWelcomeModal, hideWelcomeThisSession]);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && visible) {
        hide();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [visible, hide]);

  function handleOpenExample(example: Example) {
    incrementProjectsCreated();
    loadDocument(example.file);
    hide();
  }

  function handleStartBlank() {
    incrementProjectsCreated();
    const partId = addPrimitive("cube");
    select(partId);
    setTransformMode("translate");
    hide();
  }

  function handleBuildWithAI() {
    incrementProjectsCreated();
    setChatOpen(true);
    hide();
    // Let the chat sidebar mount/animate in, then focus the textarea.
    window.dispatchEvent(new CustomEvent("vcad:focus-chat-input"));
  }

  function handleOpenFile() {
    fileInputRef.current?.click();
  }

  function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;

    const reader = new FileReader();
    reader.onload = (event) => {
      try {
        const content = event.target?.result as string;
        const vcadFile = parseVcadFile(content);
        loadDocument(vcadFile);
        hide();
      } catch (err) {
        console.error("Failed to parse file:", err);
      }
    };
    reader.readAsText(file);
    e.target.value = "";
  }

  // Split examples into hero (6 big cards) + rest (small pills)
  const heroExamples = HERO_EXAMPLE_IDS
    .map((id) => examples.find((ex) => ex.id === id))
    .filter((ex): ex is Example => ex != null);
  const restExamples = examples.filter((ex) => !HERO_EXAMPLE_IDS.includes(ex.id));

  return (
    <div
      className={cn(
        "absolute inset-0 z-20 flex items-center justify-center pointer-events-none",
        "transition-opacity duration-300",
        visible ? "opacity-100" : "opacity-0",
      )}
    >
      {/* Subtle backdrop — blurs the viewport without hiding it entirely */}
      <div
        className={cn(
          "absolute inset-0 bg-bg/60 backdrop-blur-sm",
          "transition-opacity duration-300",
          visible ? "opacity-100" : "opacity-0",
        )}
      />

      <div
        className={cn(
          "relative border border-border bg-surface/95 backdrop-blur-md shadow-2xl",
          "w-[560px] max-w-[92vw]",
          "transition-all duration-300 ease-out",
          visible ? "scale-100 translate-y-0 pointer-events-auto" : "scale-95 translate-y-2 pointer-events-none",
        )}
      >
        {/* Hidden file input */}
        <input
          ref={fileInputRef}
          type="file"
          accept=".vcad,.loon,.json,.step,.stp,.stl"
          onChange={handleFileChange}
          className="hidden"
        />

        {/* Close */}
        <button
          onClick={hide}
          aria-label="Dismiss welcome"
          className="absolute right-2 top-2 z-10 p-1.5 text-text-muted hover:bg-border/50 hover:text-text cursor-pointer rounded"
        >
          <X size={14} />
        </button>

        {/* Hero */}
        <div className="flex flex-col items-center px-8 pt-9 pb-5">
          <h1 className="text-4xl font-bold tracking-tighter text-text leading-none">
            vcad<span className="text-brand">.</span>
          </h1>
          <p className="mt-2 text-xs text-text-muted">
            parametric CAD, free in your browser
          </p>
        </div>

        {/* Example grid — primary happy path */}
        <div className="px-6 pb-4">
          <div className="mb-2 flex items-baseline justify-between">
            <span className="text-[10px] font-medium uppercase tracking-[0.12em] text-text-muted">
              Start with an example
            </span>
            <span className="text-[10px] text-text-muted opacity-70">
              drag any parameter to modify
            </span>
          </div>
          <div className="grid grid-cols-3 gap-1.5">
            {heroExamples.map((example) => {
              const meta = EXAMPLE_ICONS[example.id];
              const Icon = meta?.Icon;
              return (
                <button
                  key={example.id}
                  onClick={() => handleOpenExample(example)}
                  className={cn(
                    "group flex flex-col items-center gap-1.5",
                    "border border-border bg-bg/40 hover:bg-card hover:border-text-muted/50",
                    "px-3 py-4 transition-colors cursor-pointer",
                    "focus:outline-none focus:ring-1 focus:ring-brand/70",
                  )}
                >
                  {Icon && (
                    <Icon
                      size={22}
                      weight="regular"
                      className={cn(
                        meta?.color,
                        "transition-transform group-hover:scale-110",
                      )}
                    />
                  )}
                  <span className="text-xs text-text">{example.name}</span>
                </button>
              );
            })}
          </div>
          {restExamples.length > 0 && (
            <div className="mt-2 flex flex-wrap justify-center gap-x-3 gap-y-1">
              {restExamples.map((example) => (
                <button
                  key={example.id}
                  onClick={() => handleOpenExample(example)}
                  className="text-[11px] text-text-muted hover:text-text cursor-pointer"
                >
                  {example.name}
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Divider */}
        <div className="relative px-6 py-1">
          <div className="flex items-center gap-3">
            <div className="flex-1 h-px bg-border" />
            <span className="text-[10px] text-text-muted">or</span>
            <div className="flex-1 h-px bg-border" />
          </div>
        </div>

        {/* AI hero CTA — differentiated action gets the full-width slot */}
        <div className="px-6 pt-3">
          <button
            onClick={handleBuildWithAI}
            className={cn(
              "group relative w-full overflow-hidden",
              "flex items-center justify-between gap-3",
              "px-4 py-3 text-left",
              "border border-brand/40 bg-gradient-to-r from-brand/10 via-brand/5 to-transparent",
              "hover:border-brand hover:from-brand/20 hover:via-brand/10",
              "transition-all cursor-pointer",
              "focus:outline-none focus:ring-1 focus:ring-brand",
            )}
          >
            {/* Subtle animated sheen */}
            <span
              aria-hidden
              className={cn(
                "pointer-events-none absolute inset-y-0 -left-1/2 w-1/3 -skew-x-12",
                "bg-gradient-to-r from-transparent via-brand/15 to-transparent",
                "translate-x-0 group-hover:translate-x-[400%] transition-transform duration-1000",
              )}
            />
            <div className="relative flex items-center gap-2.5 min-w-0">
              <Sparkle
                size={18}
                weight="fill"
                className="shrink-0 text-brand transition-transform group-hover:rotate-12"
              />
              <div className="flex flex-col min-w-0">
                <span className="text-sm font-medium text-text leading-tight">
                  Build with AI
                </span>
                <span className="text-[10px] text-text-muted truncate">
                  describe what you want — the AI has real CAD tools
                </span>
              </div>
            </div>
            <span className="relative shrink-0 text-xs text-brand/80 group-hover:text-brand transition-colors">
              →
            </span>
          </button>
        </div>

        {/* Secondary actions */}
        <div className="grid grid-cols-2 gap-1.5 px-6 py-3">
          <ActionButton
            icon={Plus}
            label="Start blank"
            hint="empty canvas"
            onClick={handleStartBlank}
          />
          <ActionButton
            icon={FolderOpen}
            label="Open file"
            hint=".vcad, .step, .stl"
            onClick={handleOpenFile}
          />
        </div>

        {/* Footer */}
        <div className="border-t border-border px-5 py-2.5 flex items-center justify-between gap-3 text-[10px] text-text-muted">
          <div className="flex items-center gap-1 min-w-0">
            {authEnabled ? (
              isAuthenticated ? (
                <span className="truncate">
                  signed in as{" "}
                  <span className="text-text">{user?.email}</span>
                </span>
              ) : (
                <>
                  <button
                    onClick={() => setShowAuthModal(true)}
                    className="text-text-muted hover:text-text"
                  >
                    sign in
                  </button>
                  <span className="opacity-60">to sync across devices</span>
                </>
              )
            ) : (
              <span className="opacity-60">
                your work is saved locally in this browser
              </span>
            )}
          </div>
          <label className="flex shrink-0 items-center gap-1.5 cursor-pointer">
            <input
              type="checkbox"
              checked={dontShowAgain}
              onChange={(e) => setDontShowAgain(e.target.checked)}
              className="accent-brand w-3 h-3"
            />
            don't show again
          </label>
        </div>
      </div>

      <AuthModal open={showAuthModal} onOpenChange={setShowAuthModal} />
    </div>
  );
}

function ActionButton({
  icon: Icon,
  label,
  hint,
  onClick,
}: {
  icon: Icon;
  label: string;
  hint: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "group flex flex-col items-start gap-0.5",
        "border border-border bg-bg/40 hover:bg-card hover:border-text-muted/50",
        "px-3 py-2.5 transition-colors cursor-pointer text-left",
        "focus:outline-none focus:ring-1 focus:ring-brand/70",
      )}
    >
      <div className="flex items-center gap-1.5">
        <Icon
          size={13}
          weight="bold"
          className="text-text-muted group-hover:text-text transition-colors"
        />
        <span className="text-xs text-text">{label}</span>
      </div>
      <span className="text-[10px] text-text-muted opacity-80">{hint}</span>
    </button>
  );
}
