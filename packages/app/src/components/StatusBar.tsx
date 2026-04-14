import { FloppyDisk } from "@phosphor-icons/react/dist/ssr/FloppyDisk";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import { Command } from "@phosphor-icons/react/dist/ssr/Command";
import { CubeTransparent } from "@phosphor-icons/react/dist/ssr/CubeTransparent";
import { ChatDots } from "@phosphor-icons/react/dist/ssr/ChatDots";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { useDocumentStore, useUiStore, useChatStore } from "@vcad/core";
import { cn } from "@/lib/utils";

interface StatusBarProps {
  onSave: () => void;
  onOpen: () => void;
  onAboutOpen: () => void;
}

/**
 * Classic Borland / Turbo Vision function-key hint row.
 *
 * Each hint is clickable (same action as pressing the F-key itself) and
 * keyboard bindings are wired separately in useKeyboardShortcuts. Right side
 * shows live document info (part count, selection) like the status line in
 * Delphi / C++ Builder.
 */
export function StatusBar({ onSave, onOpen, onAboutOpen }: StatusBarProps) {
  const parts = useDocumentStore((s) => s.parts);
  const isDirty = useDocumentStore((s) => s.isDirty);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const toggleWireframe = useUiStore((s) => s.toggleWireframe);

  const handleCommandPalette = () => {
    useUiStore.getState().setCommandPaletteOpen(true);
  };

  const hints: Array<{
    key: string;
    label: string;
    icon?: React.ComponentType<{ size?: number; className?: string }>;
    onClick: () => void;
  }> = [
    { key: "F1", label: "Help", icon: Info, onClick: onAboutOpen },
    { key: "F2", label: "Save", icon: FloppyDisk, onClick: onSave },
    { key: "F3", label: "Open", icon: FolderOpen, onClick: onOpen },
    { key: "F5", label: "Wireframe", icon: CubeTransparent, onClick: toggleWireframe },
    {
      key: "F6",
      label: "Right Sidebar",
      icon: ChatDots,
      onClick: () => useChatStore.getState().toggleOpen(),
    },
    { key: "F10", label: "Palette", icon: Command, onClick: handleCommandPalette },
  ];

  const selCount = selectedPartIds.size;

  return (
    <div className="flex h-6 items-stretch bg-surface text-[10px] font-mono select-none">
      {/* F-key hint row */}
      <div className="flex items-stretch">
        {hints.map((h) => (
          <button
            key={h.key}
            onClick={h.onClick}
            className={cn(
              "flex items-center gap-1 px-2",
              "text-text-muted hover:bg-hover hover:text-text",
              "border-r border-border/30",
            )}
            title={h.label}
          >
            <span className="text-brand font-bold">{h.key}</span>
            {h.icon && <h.icon size={11} className="opacity-80" />}
            <span>{h.label}</span>
          </button>
        ))}
      </div>

      {/* Spacer */}
      <div className="flex-1" />

      {/* Right info cluster: doc state */}
      <div className="flex items-center gap-3 px-3 text-text-muted">
        {isDirty && (
          <span className="text-brand flex items-center gap-1">
            <ArrowsClockwise size={11} />
            modified
          </span>
        )}
        <span>{parts.length} {parts.length === 1 ? "part" : "parts"}</span>
        {selCount > 0 && (
          <span className="text-brand">
            {selCount} selected
          </span>
        )}
      </div>
    </div>
  );
}
