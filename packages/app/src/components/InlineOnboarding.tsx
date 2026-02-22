import { useState, useRef, useEffect } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { useDocumentStore, useUiStore, parseVcadFile } from "@vcad/core";
import { useAuth, isAuthEnabled, getSupabase, AuthModal } from "@vcad/auth";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { examples } from "@/data/examples";
import type { Example } from "@/data/examples";

interface InlineOnboardingProps {
  visible: boolean;
}

export function InlineOnboarding({ visible }: InlineOnboardingProps) {
  const [dontShowAgain, setDontShowAgain] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const dismissWelcomeModal = useOnboardingStore((s) => s.dismissWelcomeModal);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const loadDocument = useDocumentStore((s) => s.loadDocument);
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const select = useUiStore((s) => s.select);
  const setTransformMode = useUiStore((s) => s.setTransformMode);
  const incrementProjectsCreated = useOnboardingStore(
    (s) => s.incrementProjectsCreated
  );

  const { user, isAuthenticated } = useAuth();
  const authEnabled = isAuthEnabled();
  const [showAuthModal, setShowAuthModal] = useState(false);

  // TODO: uncomment OAuth buttons below when providers are configured
  // @ts-expect-error — unused until OAuth is enabled
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  function handleOAuthSignIn(provider: "google" | "github") {
    const supabase = getSupabase();
    if (!supabase) return;
    supabase.auth.signInWithOAuth({
      provider,
      options: { redirectTo: window.location.origin },
    });
  }

  // Close on Escape
  const show = visible && !dismissed;
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && show) {
        handleDismiss();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [show]);

  function handleDismiss() {
    if (dontShowAgain) {
      dismissWelcomeModal();
    }
    setDismissed(true);
  }

  const startGuidedFlow = useOnboardingStore((s) => s.startGuidedFlow);
  const skipGuidedFlow = useOnboardingStore((s) => s.skipGuidedFlow);

  function handleNewProject() {
    incrementProjectsCreated();
    startGuidedFlow();
    setDismissed(true);
  }

  function handleSkipTutorial() {
    incrementProjectsCreated();
    skipGuidedFlow();
    const partId = addPrimitive("cube");
    select(partId);
    setTransformMode("translate");
    setDismissed(true);
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
      } catch (err) {
        console.error("Failed to parse file:", err);
      }
    };
    reader.readAsText(file);
    e.target.value = "";
  }

  function handleOpenExample(example: Example) {
    loadDocument(example.file);
  }

  return (
    <div
      className={cn(
        "absolute inset-0 z-10 flex items-center justify-center pointer-events-none",
        "transition-opacity duration-300",
        show ? "opacity-100" : "opacity-0"
      )}
    >
      <div
        className={cn(
          "relative border border-border bg-card/95 backdrop-blur-sm shadow-lg",
          "transition-all duration-300",
          show ? "scale-100 pointer-events-auto" : "scale-95 pointer-events-none"
        )}
      >
        {/* Hidden file input */}
        <input
          ref={fileInputRef}
          type="file"
          accept=".vcad,.json"
          onChange={handleFileChange}
          className="hidden"
        />

        {/* Close button */}
        <div className="absolute right-2 top-2 z-10">
          <button
            onClick={handleDismiss}
            aria-label="Dismiss onboarding"
            className="p-1 text-text-muted hover:bg-border/50 hover:text-text cursor-pointer"
          >
            <X size={14} />
          </button>
        </div>

        {/* Content */}
        <div className="flex flex-col items-center px-6 py-5">
          {/* Header */}
          <h1 className="text-2xl font-bold tracking-tighter text-text mb-0.5">
            vcad<span className="text-accent">.</span>
          </h1>
          <p className="text-xs text-text-muted mb-5">
            free parametric cad for everyone
          </p>

          {/* Action buttons */}
          <div className="flex flex-col items-center gap-2 mb-5">
            <div className="flex gap-2">
              <Button
                variant="default"
                size="sm"
                onClick={handleNewProject}
                className="gap-1.5"
              >
                <Plus size={14} weight="bold" />
                New Project
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={handleOpenFile}
                className="gap-1.5"
              >
                <FolderOpen size={14} />
                Open File
              </Button>
            </div>
            <button
              onClick={handleSkipTutorial}
              className="text-[10px] text-text-muted hover:text-text"
            >
              skip tutorial
            </button>
          </div>

          {/* Sign in */}
          {authEnabled && (
            <div className="flex flex-col items-center mb-5">
              {isAuthenticated ? (
                <p className="text-[10px] text-text-muted">
                  signed in as{" "}
                  <span className="text-text">{user?.email}</span>
                </p>
              ) : (
                <>
                  <div className="flex items-center gap-3 mb-3 w-full max-w-[220px]">
                    <div className="flex-1 h-px bg-border" />
                    <span className="text-[10px] text-text-muted">
                      or
                    </span>
                    <div className="flex-1 h-px bg-border" />
                  </div>
                  {/* OAuth buttons — uncomment when providers are configured
                  <div className="flex flex-col gap-1.5 w-full max-w-[220px]">
                    <button
                      onClick={() => handleOAuthSignIn("google")}
                      className="flex items-center gap-2 px-3 py-1.5 text-xs text-text border border-border hover:bg-border/50 cursor-pointer"
                    >
                      <svg viewBox="0 0 24 24" className="w-3.5 h-3.5" aria-hidden="true">
                        <path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 0 1-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z" />
                        <path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" />
                        <path fill="#FBBC05" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18A10.96 10.96 0 0 0 1 12c0 1.77.42 3.45 1.18 4.93l3.66-2.84z" />
                        <path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" />
                      </svg>
                      Continue with Google
                    </button>
                    <button
                      onClick={() => handleOAuthSignIn("github")}
                      className="flex items-center gap-2 px-3 py-1.5 text-xs text-text border border-border hover:bg-border/50 cursor-pointer"
                    >
                      <svg viewBox="0 0 24 24" className="w-3.5 h-3.5 fill-current" aria-hidden="true">
                        <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844a9.59 9.59 0 0 1 2.504.337c1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.02 10.02 0 0 0 22 12.017C22 6.484 17.522 2 12 2z" />
                      </svg>
                      Continue with GitHub
                    </button>
                  </div>
                  */}
                  <button
                    onClick={() => setShowAuthModal(true)}
                    className="text-[10px] text-text-muted hover:text-text"
                  >
                    sign in
                  </button>
                </>
              )}
            </div>
          )}

          {/* Examples */}
          <p className="text-[10px] text-text-muted mb-2">Try an example:</p>
          <div className="flex flex-wrap justify-center gap-x-3 gap-y-1 max-w-xs">
            {examples.map((example) => (
              <button
                key={example.id}
                onClick={() => handleOpenExample(example)}
                className="text-xs text-text-muted hover:text-text cursor-pointer"
              >
                {example.name}
              </button>
            ))}
          </div>
        </div>

        {/* Footer with checkbox */}
        <div className="border-t border-border px-4 py-2.5 flex items-center justify-center">
          <label className="flex items-center gap-1.5 text-[10px] text-text-muted cursor-pointer">
            <input
              type="checkbox"
              checked={dontShowAgain}
              onChange={(e) => setDontShowAgain(e.target.checked)}
              className="accent-accent w-3 h-3"
            />
            Don't show again
          </label>
        </div>
      </div>

      <AuthModal open={showAuthModal} onOpenChange={setShowAuthModal} />
    </div>
  );
}
