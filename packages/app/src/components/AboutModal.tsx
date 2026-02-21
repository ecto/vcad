import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { GithubLogo } from "@phosphor-icons/react/dist/ssr/GithubLogo";
import { Book } from "@phosphor-icons/react/dist/ssr/Book";
import { ChatCircle } from "@phosphor-icons/react/dist/ssr/ChatCircle";
import { cn } from "@/lib/utils";
import { useDocumentStore, useSketchStore, useUiStore } from "@vcad/core";
import { useChangelogStore } from "@/stores/changelog-store";

const QUICK_ACTIONS = [
  {
    id: "primitive",
    icon: Cube,
    label: "Add a shape",
    desc: "Box, cylinder, sphere",
    color: "text-emerald-400",
  },
  {
    id: "sketch",
    icon: PencilSimple,
    label: "Start a sketch",
    desc: "Draw 2D → extrude to 3D",
    color: "text-blue-400",
  },
  {
    id: "open",
    icon: FolderOpen,
    label: "Open file",
    desc: ".vcad, .step, .stl",
    color: "text-amber-400",
  },
  {
    id: "tutorial",
    icon: Play,
    label: "Quick tutorial",
    desc: "Learn the basics",
    color: "text-violet-400",
  },
];

const KEY_HINTS = [
  { keys: "⌘K", desc: "Chat & commands" },
  { keys: "1-7", desc: "Switch toolbar tabs" },
  { keys: "M R S", desc: "Move, Rotate, Scale" },
  { keys: "⌘⇧U D I", desc: "Boolean ops (2 selected)" },
];

export function AboutModal({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const select = useUiStore((s) => s.select);
  const setTransformMode = useUiStore((s) => s.setTransformMode);
  const enterSketchMode = useSketchStore((s) => s.enterSketchMode);
  const openWhatsNew = useChangelogStore((s) => s.openPanel);

  function handleAction(id: string) {
    onOpenChange(false);

    switch (id) {
      case "primitive":
        const partId = addPrimitive("cube");
        select(partId);
        setTransformMode("translate");
        break;
      case "sketch":
        enterSketchMode("XY");
        break;
      case "open":
        window.dispatchEvent(new CustomEvent("vcad:open"));
        break;
      case "tutorial":
        window.dispatchEvent(new CustomEvent("vcad:start-tutorial"));
        break;
    }
  }

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 w-full max-w-md -translate-x-1/2 -translate-y-1/2",
            "bg-surface p-8 shadow-2xl",
            "focus:outline-none",
          )}
        >
          {/* Close button */}
          <Dialog.Close className="absolute right-4 top-4 p-2 text-text-muted hover:text-text transition-colors cursor-pointer">
            <X size={16} />
          </Dialog.Close>

          {/* Header */}
          <div className="text-center mb-8">
            <Dialog.Title className="text-3xl font-bold tracking-tight text-text mb-2">
              vcad<span className="text-accent">.</span>
            </Dialog.Title>
            <p className="text-sm text-text-muted">
              Parametric CAD for everyone
            </p>
          </div>

          {/* Quick actions */}
          <div className="grid grid-cols-2 gap-3 mb-8">
            {QUICK_ACTIONS.map((action) => (
              <button
                key={action.id}
                onClick={() => handleAction(action.id)}
                className={cn(
                  "flex flex-col items-start gap-1 p-4",
                  "bg-bg hover:bg-hover transition-colors text-left",
                )}
              >
                <action.icon size={20} className={action.color} />
                <span className="text-sm font-medium text-text">{action.label}</span>
                <span className="text-xs text-text-muted">{action.desc}</span>
              </button>
            ))}
          </div>

          {/* Keyboard hints */}
          <div className="flex flex-wrap justify-center gap-x-4 gap-y-2 mb-8 text-xs text-text-muted">
            {KEY_HINTS.map((hint) => (
              <div key={hint.keys} className="flex items-center gap-1.5">
                <kbd className="px-1.5 py-0.5 bg-bg text-[10px] font-mono">
                  {hint.keys}
                </kbd>
                <span>{hint.desc}</span>
              </div>
            ))}
          </div>

          {/* Footer links */}
          <div className="flex items-center justify-center gap-6 text-xs">
            <a
              href="https://github.com/nicholaschuayunzhi/vcad"
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1.5 text-text-muted hover:text-text transition-colors"
            >
              <GithubLogo size={14} />
              <span>GitHub</span>
            </a>
            <a
              href="https://vcad.io/docs"
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1.5 text-text-muted hover:text-text transition-colors"
            >
              <Book size={14} />
              <span>Docs</span>
            </a>
            <a
              href="https://discord.gg/vcad"
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1.5 text-text-muted hover:text-text transition-colors"
            >
              <ChatCircle size={14} />
              <span>Discord</span>
            </a>
            <button
              onClick={() => {
                openWhatsNew();
                onOpenChange(false);
              }}
              className="text-text-muted/50 hover:text-accent transition-colors"
              title="What's new"
            >
              v{__APP_VERSION__}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
