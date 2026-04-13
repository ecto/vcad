import { useState, useEffect, type ReactNode, Suspense, lazy } from "react";
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
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { FloppyDisk } from "@phosphor-icons/react/dist/ssr/FloppyDisk";
import { FilePlus } from "@phosphor-icons/react/dist/ssr/FilePlus";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import {
  useDocumentStore,
  useUiStore,
  useChatStore,
  useSketchStore,
  type PrimitiveKind,
} from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { SignInButton, UserMenu, triggerSync } from "@vcad/auth";
import { cn } from "@/lib/utils";
import { BottomSheet } from "./BottomSheet";
import { FeatureTree } from "@/components/FeatureTree";

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
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const selection = selectedPartIds.size;

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
            vcad<span className="text-accent">.</span>
          </span>
          <span className="text-xs text-text-muted truncate max-w-[120px]">
            {docName ?? "Untitled"}
          </span>
          {isDirty && <span className="text-accent text-xs">*</span>}
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
        <div className="p-2">
          <SheetSection>File</SheetSection>
          <SheetRow icon={FilePlus} label="New"
            onClick={() => {
              if (useDocumentStore.getState().isDirty && !window.confirm("Discard unsaved changes?")) return;
              useDocumentStore.getState().newDocument(crypto.randomUUID(), "Untitled");
              setMenuOpen(false);
            }}
          />
          <SheetRow icon={FolderOpen} label="Open…" onClick={() => { onOpen(); setMenuOpen(false); }} />
          <SheetRow icon={FloppyDisk} label="Save" onClick={() => { onSave(); setMenuOpen(false); }} />
          <SheetSection>Edit</SheetSection>
          <SheetRow icon={ArrowCounterClockwise} label="Undo" onClick={() => { undo(); setMenuOpen(false); }} />
          <SheetRow icon={ArrowClockwise} label="Redo" onClick={() => { redo(); setMenuOpen(false); }} />
          <SheetRow icon={Copy} label="Duplicate Selection"
            onClick={() => {
              const ids = Array.from(useUiStore.getState().selectedPartIds);
              if (ids.length === 0) return;
              const newIds = useDocumentStore.getState().duplicateParts(ids);
              useUiStore.getState().selectMultiple(newIds);
              setMenuOpen(false);
            }}
          />
          <SheetRow icon={Trash} label="Delete Selection"
            onClick={() => {
              const { selectedPartIds, clearSelection } = useUiStore.getState();
              for (const id of selectedPartIds) useDocumentStore.getState().removePart(id);
              clearSelection();
              setMenuOpen(false);
            }}
          />
          <SheetSection>Help</SheetSection>
          <SheetRow icon={Info} label="About vcad" onClick={() => { onAboutOpen(); setMenuOpen(false); }} />
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
        active && "text-accent",
        primary && "text-accent",
      )}
    >
      <Icon size={22} weight={primary ? "bold" : "regular"} />
      <span className="text-[10px] leading-none">{label}</span>
    </button>
  );
}

function SheetSection({ children }: { children: ReactNode }) {
  return (
    <div className="px-3 pt-3 pb-1 text-[10px] font-bold uppercase tracking-wider text-text-muted">
      {children}
    </div>
  );
}

function SheetRow({
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
      className="flex w-full items-center gap-3 px-3 min-h-11 text-sm text-text active:bg-hover"
    >
      <Icon size={18} />
      <span>{label}</span>
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
