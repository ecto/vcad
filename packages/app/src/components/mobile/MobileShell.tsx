import { useState, useEffect, useCallback, type ReactNode, Suspense, lazy } from "react";
import { List as MenuIcon } from "@phosphor-icons/react/dist/ssr/List";
import { TreeStructure } from "@phosphor-icons/react/dist/ssr/TreeStructure";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { ChatDots } from "@phosphor-icons/react/dist/ssr/ChatDots";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";
import { ArrowClockwise } from "@phosphor-icons/react/dist/ssr/ArrowClockwise";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Cylinder } from "@phosphor-icons/react/dist/ssr/Cylinder";
import { Sphere } from "@phosphor-icons/react/dist/ssr/Sphere";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import {
  useDocumentStore,
  useUiStore,
  useChatStore,
  useSketchStore,
  COMMAND_CATEGORIES,
  CATEGORY_LABELS,
  CATEGORY_ICON_COLORS,
  type Command,
  type PrimitiveKind,
} from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { useChangelogStore } from "@/stores/changelog-store";
import { SignInButton, UserMenu, triggerSync } from "@vcad/auth";
import { cn } from "@/lib/utils";
import { BottomSheet } from "./BottomSheet";
import { FeatureTree } from "@/components/FeatureTree";
import { useAppCommands } from "@/hooks/useAppCommands";
import { COMMAND_ICONS } from "@/lib/command-icons";

const PropertyPanel = lazy(() =>
  import("@/components/PropertyPanel").then((m) => ({ default: m.PropertyPanel })),
);
const ChatSidebar = lazy(() =>
  import("@/components/ChatSidebar").then((m) => ({ default: m.ChatSidebar })),
);

interface MobileShellProps {
  onAboutOpen: () => void;
  onSave: () => void;
  onOpen: () => void;
  /** Viewport content. */
  children: ReactNode;
}

/**
 * Mobile IDE shell. Top bar with doc name + menu, big viewport, bottom dock
 * with five thumb-sized primary actions. Sidebars and menus are bottom sheets.
 */
export function MobileShell({ onAboutOpen, onSave, onOpen, children }: MobileShellProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [treeOpen, setTreeOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [chatSheetOpen, setChatSheetOpen] = useState(false);

  const isDirty = useDocumentStore((s) => s.isDirty);
  const docName = useDocumentStore((s) => s.documentName);
  const undo = useDocumentStore((s) => s.undo);
  const redo = useDocumentStore((s) => s.redo);
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  // Subscribing so enabled-state changes trigger a re-render of the menu
  // rows (which call cmd.enabled?.() inline). The values themselves are read
  // via getState() inside the useAppCommands actions.
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const parts = useDocumentStore((s) => s.parts);
  const unreadChangelog = useChangelogStore((s) => s.getUnreadCount());
  const selection = selectedPartIds.size;
  void parts;

  const dismissMenu = useCallback(() => setMenuOpen(false), []);
  const commands = useAppCommands({
    onDismiss: dismissMenu,
    onAboutOpen,
    onSave,
    onOpen,
    surface: "mobile-menu",
  });

  // Chat opens only on explicit action — dock button, ⌘K / F6 event, or
  // code that dispatches `vcad:open-chat`. Never mirrors the store's default
  // `open: true` (which is right for desktop sidebars, wrong for mobile sheets).
  const handleChatSheetChange = (next: boolean) => {
    setChatSheetOpen(next);
    useChatStore.getState().setOpen(next);
  };
  useEffect(() => {
    // Force the store closed on mount so the mobile shell starts with no sheet.
    if (useChatStore.getState().open) useChatStore.getState().setOpen(false);
    const handleOpen = () => setChatSheetOpen(true);
    window.addEventListener("vcad:open-chat", handleOpen);
    return () => window.removeEventListener("vcad:open-chat", handleOpen);
  }, []);

  const handleCreate = (kind: PrimitiveKind) => {
    addPrimitive(kind);
    setCreateOpen(false);
  };

  return (
    <div className="flex h-[100dvh] w-screen flex-col overflow-hidden bg-bg">
      {/* ──── Top bar ────────────────────────────────── */}
      <div className="flex h-12 shrink-0 items-center border-b border-border bg-surface pl-3 pr-1">
        <button
          onClick={() => setMenuOpen(true)}
          className="flex h-10 w-10 -ml-2 items-center justify-center text-text-muted hover:text-text"
          aria-label="Menu"
        >
          <MenuIcon size={22} />
        </button>
        <div className="flex items-center gap-1 ml-1 min-w-0">
          <span className="text-sm font-bold tracking-tighter text-text">
            vcad<span className="text-brand">.</span>
          </span>
          <span className="text-xs text-text-muted truncate max-w-[120px]">
            {docName ?? "Untitled"}
          </span>
          {isDirty && <span className="text-brand text-xs">*</span>}
        </div>
        <div className="flex-1" />
        <button
          onClick={undo}
          className="flex h-10 w-10 items-center justify-center text-text-muted active:text-text"
          aria-label="Undo"
        >
          <ArrowCounterClockwise size={20} />
        </button>
        <button
          onClick={redo}
          className="flex h-10 w-10 items-center justify-center text-text-muted active:text-text"
          aria-label="Redo"
        >
          <ArrowClockwise size={20} />
        </button>
        <SignInButton
          variant="icon-text"
          className="flex items-center justify-center h-10 px-2 text-xs text-text-muted"
        />
        <UserMenu onSyncNow={() => triggerSync()} />
      </div>

      {/* ──── Viewport ───────────────────────────────── */}
      <div className="relative flex-1 min-h-0">
        {children}

        {/* Selection property sheet — always mounted, auto-shows when selection > 0. */}
        {selection > 0 && (
          <div className="pointer-events-none fixed inset-x-0 bottom-[calc(56px+env(safe-area-inset-bottom))] z-30 flex justify-center">
            <div className="pointer-events-auto w-full max-h-[55dvh] bg-surface border-t border-border shadow-[0_-4px_16px_rgba(0,0,0,0.3)] flex flex-col">
              <Suspense fallback={null}>
                <PropertyPanel />
              </Suspense>
            </div>
          </div>
        )}
      </div>

      {/* ──── Bottom dock ────────────────────────────── */}
      <div
        className={cn(
          "flex h-14 shrink-0 items-stretch border-t border-border bg-surface",
          "pb-[env(safe-area-inset-bottom)]",
        )}
      >
        <DockButton
          icon={TreeStructure}
          label="Tree"
          onClick={() => setTreeOpen(true)}
        />
        <DockButton
          icon={Plus}
          label="Create"
          primary
          onClick={() => setCreateOpen(true)}
        />
        <DockButton
          icon={ChatDots}
          label="Chat"
          onClick={() => handleChatSheetChange(true)}
        />
        <DockButton
          icon={ArrowsOutCardinal}
          label="Fit"
          onClick={() => window.dispatchEvent(new CustomEvent("vcad:camera-fit"))}
        />
      </div>

      {/* ──── Sheets ─────────────────────────────────── */}
      <BottomSheet open={menuOpen} onOpenChange={setMenuOpen} title="Menu">
        <div className="py-1">
          {COMMAND_CATEGORIES.map((cat) => {
            const inCat = commands.filter((c) => c.category === cat);
            if (inCat.length === 0) return null;
            return (
              <div key={cat}>
                <SheetSection>{CATEGORY_LABELS[cat]}</SheetSection>
                {inCat.map((cmd) => (
                  <CommandRow
                    key={cmd.id}
                    command={cmd}
                    badge={cmd.id === "whats-new" && unreadChangelog > 0 ? unreadChangelog : undefined}
                  />
                ))}
              </div>
            );
          })}
        </div>
      </BottomSheet>

      <BottomSheet open={treeOpen} onOpenChange={setTreeOpen} title="Feature Tree" size="full">
        <div className="h-full">
          <FeatureTree />
        </div>
      </BottomSheet>

      <BottomSheet open={createOpen} onOpenChange={setCreateOpen} title="Create">
        <div className="grid grid-cols-3 gap-2 p-3">
          <CreateTile icon={Cube} label="Box" onClick={() => handleCreate("cube")} />
          <CreateTile icon={Cylinder} label="Cylinder" onClick={() => handleCreate("cylinder")} />
          <CreateTile icon={Sphere} label="Sphere" onClick={() => handleCreate("sphere")} />
          <CreateTile icon={PencilSimple} label="Sketch" onClick={() => {
            useSketchStore.getState().enterFaceSelectionMode();
            useNotificationStore.getState().addToast("Select a face to sketch on", "info");
            setCreateOpen(false);
          }} />
        </div>
      </BottomSheet>

      <BottomSheet open={chatSheetOpen} onOpenChange={handleChatSheetChange} title="AI Chat" size="full">
        <div className="h-full flex flex-col">
          <Suspense fallback={null}>
            <ChatSidebar />
          </Suspense>
        </div>
      </BottomSheet>
    </div>
  );
}

function DockButton({
  icon: Icon,
  label,
  onClick,
  active,
  primary,
}: {
  icon: React.ComponentType<{ size?: number; weight?: "bold" | "regular" }>;
  label: string;
  onClick: () => void;
  active?: boolean;
  primary?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex flex-1 flex-col items-center justify-center gap-0.5 min-h-11",
        "text-text-muted active:bg-hover",
        active && "text-brand",
        primary && "text-brand",
      )}
    >
      <Icon size={22} weight={primary ? "bold" : "regular"} />
      <span className="text-[10px] leading-none">{label}</span>
    </button>
  );
}

function SheetSection({ children }: { children: ReactNode }) {
  return (
    <div className="px-4 pt-4 pb-1 text-[10px] font-bold uppercase tracking-[0.12em] text-text-muted">
      {children}
    </div>
  );
}

/** Defensive wrapper around command.enabled() — a throwing check (e.g. the
 * kernel WASM falling over) should NOT crash the whole app via the error
 * boundary. Treat any exception as "disabled" and keep the menu rendering. */
function safeEnabled(command: Command): boolean {
  if (!command.enabled) return true;
  try {
    return command.enabled();
  } catch {
    return false;
  }
}

function CommandRow({
  command,
  badge,
}: {
  command: Command;
  badge?: number;
}) {
  const iconName = command.dynamicIcon?.() ?? command.icon;
  const label = command.dynamicLabel?.() ?? command.label;
  const Icon = COMMAND_ICONS[iconName];
  const enabled = safeEnabled(command);
  const iconColor = command.category
    ? CATEGORY_ICON_COLORS[command.category]
    : "text-text-muted";
  return (
    <button
      onClick={command.action}
      disabled={!enabled}
      className={cn(
        "flex w-full items-center gap-3 px-4 min-h-11 text-sm text-text active:bg-hover transition-colors",
        !enabled && "opacity-40",
      )}
    >
      <span
        className={cn(
          "flex h-7 w-7 shrink-0 items-center justify-center rounded bg-bg/50",
          iconColor,
        )}
      >
        {Icon ? <Icon size={16} weight="regular" /> : null}
      </span>
      <span className="flex-1 text-left">{label}</span>
      {badge !== undefined && badge > 0 && (
        <span className="min-w-[18px] rounded-full bg-brand px-1.5 py-0.5 text-center text-[10px] font-bold text-white">
          {badge}
        </span>
      )}
      {command.shortcut && (
        <kbd className="font-mono text-[10px] text-text-muted">{command.shortcut}</kbd>
      )}
    </button>
  );
}

function CreateTile({
  icon: Icon,
  label,
  onClick,
}: {
  icon: React.ComponentType<{ size?: number }>;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className="flex aspect-square flex-col items-center justify-center gap-2 border border-border bg-card active:bg-hover rounded"
    >
      <Icon size={32} />
      <span className="text-xs text-text">{label}</span>
    </button>
  );
}
