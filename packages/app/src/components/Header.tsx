import { useState } from "react";
import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { Mouse } from "@phosphor-icons/react/dist/ssr/Mouse";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import * as Popover from "@radix-ui/react-popover";
import {
  useDocumentStore,
  useUiStore,
  useEngineStore,
  exportStlBlob,
  exportGltfBlob,
  exportStepBlob,
  CATEGORY_ICON_COLORS,
  type Command,
} from "@vcad/core";
import { cn } from "@/lib/utils";
import { downloadBlob } from "@/lib/download";
import { examples } from "@/data/examples";
import { CameraSettingsPanel } from "./CameraSettingsPanel";
import { useCameraSettingsStore } from "@/stores/camera-settings-store";
import { CONTROL_PRESETS } from "@/types/camera-controls";
import { SignInButton, UserMenu, triggerSync } from "@vcad/auth";
import { useChangelogStore } from "@/stores/changelog-store";
import { useNotificationStore } from "@/stores/notification-store";
import { useAppCommands } from "@/hooks/useAppCommands";
import { COMMAND_ICONS } from "@/lib/command-icons";

interface HeaderProps {
  onAboutOpen: () => void;
  onSave: () => void;
  onOpen: () => void;
  /** Tool palette (tab strip + icon row) docked directly under the menu bar. */
  children?: React.ReactNode;
}


// ---------------------------------------------------------------------------
// Borland C++ Builder / Delphi-style menu bar
// ---------------------------------------------------------------------------

/** Classic text-style menu item used in the top menu-bar row. */
function MenuBarItem({
  label,
  accelerator,
  children,
}: {
  label: string;
  /** First letter to underline, e.g. "F" for "File" */
  accelerator?: string;
  children: React.ReactNode | ((close: () => void) => React.ReactNode);
}) {
  const [open, setOpen] = useState(false);
  const renderedLabel =
    accelerator && label.startsWith(accelerator) ? (
      <>
        <span className="underline">{label[0]}</span>
        {label.slice(1)}
      </>
    ) : (
      label
    );
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          className={cn(
            "h-6 px-2 text-xs text-text hover:bg-hover transition-colors",
            open && "bg-hover",
          )}
        >
          {renderedLabel}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          sideOffset={2}
          align="start"
          className="z-50 min-w-[180px] border border-border bg-surface shadow-lg py-1"
          onCloseAutoFocus={(e) => e.preventDefault()}
        >
          {typeof children === "function"
            ? (children as (close: () => void) => React.ReactNode)(() => setOpen(false))
            : children}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

function MenuItem({
  onClick,
  children,
  shortcut,
  icon: Icon,
  iconClassName,
  disabled,
  badge,
}: {
  onClick: () => void;
  children: React.ReactNode;
  shortcut?: string;
  icon?: React.ComponentType<{ size?: number; className?: string }>;
  iconClassName?: string;
  disabled?: boolean;
  badge?: number;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover",
        disabled && "opacity-40 cursor-not-allowed",
      )}
    >
      {Icon && <Icon size={13} className={iconClassName ?? "text-text-muted"} />}
      <span className="flex-1 text-left">{children}</span>
      {badge !== undefined && badge > 0 && (
        <span className="min-w-[18px] rounded-full bg-brand px-1.5 py-0.5 text-center text-[10px] font-bold text-white">
          {badge}
        </span>
      )}
      {shortcut && <span className="text-text-muted text-[10px]">{shortcut}</span>}
    </button>
  );
}

function MenuSeparator() {
  return <div className="my-1 border-t border-border" />;
}

/** Render a registry command as a Header MenuItem. Looks the command up by
 * id, applies the category icon color, wires onClick to run the action and
 * then close the parent menu popover. Keeps Header in sync with mobile and
 * the command palette — all three surfaces render the same action list. */
/** Defensive wrapper around command.enabled() — keeps a throwing check (e.g.
 * kernel WASM in a broken state) from tripping the error boundary. */
function safeEnabled(command: Command): boolean {
  if (!command.enabled) return true;
  try {
    return command.enabled();
  } catch {
    return false;
  }
}

function CommandMenuItem({
  id,
  close,
  commands,
  label,
  badge,
}: {
  id: string;
  close: () => void;
  commands: Command[];
  /** Optional label override for commands whose display text is dynamic
   * (e.g. theme cycle, wireframe toggle). Registry's static label is used
   * when omitted. */
  label?: React.ReactNode;
  badge?: number;
}) {
  const cmd = commands.find((c) => c.id === id);
  if (!cmd) return null;
  const iconName = cmd.dynamicIcon?.() ?? cmd.icon;
  const Icon = COMMAND_ICONS[iconName];
  const enabled = safeEnabled(cmd);
  const iconColor = cmd.category
    ? CATEGORY_ICON_COLORS[cmd.category]
    : "text-text-muted";
  const displayLabel = label ?? cmd.dynamicLabel?.() ?? cmd.label;
  return (
    <MenuItem
      icon={Icon}
      iconClassName={iconColor}
      shortcut={cmd.shortcut}
      disabled={!enabled}
      badge={badge}
      onClick={() => {
        if (!enabled) return;
        cmd.action();
        close();
      }}
    >
      {displayLabel}
    </MenuItem>
  );
}

/** Ray Tracing submenu — opens to the right of the View menu with quality presets. */
function RayTracingSubmenu() {
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceQuality = useUiStore((s) => s.raytraceQuality);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const raytraceEdgesEnabled = useUiStore((s) => s.raytraceEdgesEnabled);
  const toggleRenderMode = useUiStore((s) => s.toggleRenderMode);
  const setRaytraceQuality = useUiStore((s) => s.setRaytraceQuality);
  const setRaytraceEdgesEnabled = useUiStore((s) => s.setRaytraceEdgesEnabled);

  if (!raytraceAvailable) return null;

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover">
          <Sparkle size={13} className={renderMode === "raytrace" ? "text-brand" : "text-text-muted"} />
          <span className="flex-1 text-left">Ray Tracing</span>
          <span className="text-text-muted text-[10px]">
            {renderMode === "raytrace" ? raytraceQuality : "Off"}
          </span>
          <CaretRight size={10} className="text-text-muted" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="right"
          sideOffset={0}
          align="start"
          className="z-50 min-w-[160px] border border-border bg-surface shadow-lg py-1"
        >
          <button
            onClick={() => { if (renderMode === "raytrace") toggleRenderMode(); }}
            className="flex w-full items-center px-3 py-1 text-xs hover:bg-hover"
          >
            <span className={renderMode === "standard" ? "text-brand" : "text-text"}>Off</span>
          </button>
          {(["draft", "standard", "high"] as const).map((q) => (
            <button
              key={q}
              onClick={() => {
                if (renderMode !== "raytrace") toggleRenderMode();
                setRaytraceQuality(q);
              }}
              className="flex w-full items-center px-3 py-1 text-xs hover:bg-hover"
            >
              <span
                className={
                  renderMode === "raytrace" && raytraceQuality === q ? "text-brand" : "text-text"
                }
              >
                {q.charAt(0).toUpperCase() + q.slice(1)}
              </span>
            </button>
          ))}
          <MenuSeparator />
          <button
            onClick={() => setRaytraceEdgesEnabled(!raytraceEdgesEnabled)}
            className="flex w-full items-center px-3 py-1 text-xs hover:bg-hover"
          >
            <span className={raytraceEdgesEnabled ? "text-brand" : "text-text"}>
              Edges {raytraceEdgesEnabled ? "On" : "Off"}
            </span>
          </button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** Mouse Controls submenu — opens the CameraSettingsPanel as a right-side popover. */
function MouseControlsSubmenu() {
  const controlSchemeId = useCameraSettingsStore((s) => s.controlSchemeId);
  const currentSchemeName = CONTROL_PRESETS[controlSchemeId]?.name ?? "vcad";
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover">
          <Mouse size={13} className="text-text-muted" />
          <span className="flex-1 text-left">Mouse Controls</span>
          <span className="text-text-muted text-[10px]">{currentSchemeName}</span>
          <CaretRight size={10} className="text-text-muted" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="right"
          sideOffset={0}
          align="start"
          className="z-50 w-64 border border-border bg-surface shadow-lg p-2 max-h-[80vh] overflow-y-auto"
        >
          <CameraSettingsPanel />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

export function Header({ onAboutOpen, onSave, onOpen, children }: HeaderProps) {
  const isDirty = useDocumentStore((s) => s.isDirty);
  const unreadChangelog = useChangelogStore((s) => s.getUnreadCount());
  // Subscribe to state that affects command.enabled() / dynamicLabel() /
  // dynamicIcon() results. The actions and getters themselves read via
  // getState() inside useAppCommands, but the menu surface needs to
  // re-render when these change for the labels/icons to refresh.
  useDocumentStore((s) => s.parts);
  useDocumentStore((s) => s.document);
  useUiStore((s) => s.selectedPartIds);
  useUiStore((s) => s.theme);
  useUiStore((s) => s.showWireframe);
  useUiStore((s) => s.gridSnap);

  const commands = useAppCommands({
    onDismiss: () => {
      // Each MenuBarItem owns its own close() via render prop — we close it
      // explicitly from CommandMenuItem so each popover dismisses at the
      // right time. onDismiss stays a noop here.
    },
    onAboutOpen,
    onSave,
    onOpen,
  });

  const handleCommandPalette = () => {
    useUiStore.getState().setCommandPaletteOpen(true);
  };

  const handleExport = (format: "stl" | "glb" | "step") => {
    const scene = useEngineStore.getState().scene;
    if (!scene) {
      useNotificationStore.getState().addToast("Nothing to export", "info");
      return;
    }
    try {
      const blob =
        format === "stl" ? exportStlBlob(scene)
        : format === "glb" ? exportGltfBlob(scene)
        : exportStepBlob(scene);
      downloadBlob(blob, `model.${format}`);
    } catch (err) {
      useNotificationStore
        .getState()
        .addToast(`Export failed: ${(err as Error).message}`, "error");
    }
  };

  // Examples live in /src/data and their file loader still goes through a
  // dispatched event — not in the command registry because each entry is
  // unique, not a shared command.
  const handleLoadExample = (file: unknown) => {
    window.dispatchEvent(new CustomEvent("vcad:load-example", { detail: { file } }));
  };

  return (
    <div className="flex flex-col bg-surface">
      {/* ─────────────────────────────────────────────────────── */}
      {/* Row 1: menu bar (logo + File/Edit/View/Tools/Help)     */}
      {/* ─────────────────────────────────────────────────────── */}
      <div className="relative flex h-7 items-center gap-0 px-2 border-b border-border/30">
        <div className="flex items-center gap-1 pr-3">
          <span className="text-sm font-bold tracking-tighter text-text">
            vcad<span className="text-brand">.</span>
          </span>
          {isDirty && <span className="text-brand text-xs">*</span>}
        </div>

        {/* iTunes-style center search bar — opens the screen-centered ⌘K palette */}
        <button
          onClick={handleCommandPalette}
          className={cn(
            "absolute left-1/2 -translate-x-1/2",
            "flex h-5 w-72 max-w-[40vw] items-center gap-1.5 px-2",
            "border border-border bg-bg/60 hover:bg-bg",
            "text-[11px] text-text-muted hover:text-text",
            "rounded-sm transition-colors",
          )}
        >
          <MagnifyingGlass size={11} />
          <span className="flex-1 text-left truncate">Search or ask AI…</span>
          <span className="text-[9px] text-text-muted/70 font-mono">⌘K</span>
        </button>

        <MenuBarItem label="File" accelerator="F">
          {(close: () => void) => (
            <>
              <CommandMenuItem id="new-document" close={close} commands={commands} />
              <CommandMenuItem id="open" close={close} commands={commands} />
              <CommandMenuItem id="open-cloud" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="save" close={close} commands={commands} />
              <MenuSeparator />
              <Popover.Root>
                <Popover.Trigger asChild>
                  <button className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover">
                    <Export size={13} className="text-sky-400" />
                    <span className="flex-1 text-left">Export</span>
                    <CaretRight size={10} className="text-text-muted" />
                  </button>
                </Popover.Trigger>
                <Popover.Portal>
                  <Popover.Content
                    side="right"
                    sideOffset={0}
                    align="start"
                    className="z-50 min-w-[160px] border border-border bg-surface shadow-lg py-1"
                  >
                    <MenuItem onClick={() => { handleExport("stl"); close(); }}>STL</MenuItem>
                    <MenuItem onClick={() => { handleExport("glb"); close(); }}>GLB</MenuItem>
                    <MenuItem onClick={() => { handleExport("step"); close(); }}>STEP</MenuItem>
                  </Popover.Content>
                </Popover.Portal>
              </Popover.Root>
              <MenuSeparator />
              <Popover.Root>
                <Popover.Trigger asChild>
                  <button className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover">
                    <BookOpen size={13} className="text-sky-400" />
                    <span className="flex-1 text-left">Examples</span>
                    <CaretRight size={10} className="text-text-muted" />
                  </button>
                </Popover.Trigger>
                <Popover.Portal>
                  <Popover.Content
                    side="right"
                    sideOffset={0}
                    align="start"
                    className="z-50 min-w-[200px] max-h-[60vh] overflow-y-auto border border-border bg-surface shadow-lg py-1"
                  >
                    {examples.map((ex) => (
                      <MenuItem
                        key={ex.id}
                        onClick={() => { handleLoadExample(ex.file); close(); }}
                      >
                        {ex.name}
                      </MenuItem>
                    ))}
                  </Popover.Content>
                </Popover.Portal>
              </Popover.Root>
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="Edit" accelerator="E">
          {(close: () => void) => (
            <>
              <CommandMenuItem id="undo" close={close} commands={commands} />
              <CommandMenuItem id="redo" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="copy" close={close} commands={commands} />
              <CommandMenuItem id="paste" close={close} commands={commands} />
              <CommandMenuItem id="duplicate" close={close} commands={commands} />
              <CommandMenuItem id="delete" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="select-all" close={close} commands={commands} />
              <CommandMenuItem id="deselect" close={close} commands={commands} />
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="View" accelerator="V">
          {(close: () => void) => (
            <>
              <CommandMenuItem id="toggle-sidebar" close={close} commands={commands} />
              <CommandMenuItem id="toggle-chat" close={close} commands={commands} />
              <CommandMenuItem id="toggle-status-bar" close={close} commands={commands} />
              <CommandMenuItem id="toggle-devtools" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="camera-isometric" close={close} commands={commands} />
              <CommandMenuItem id="camera-top" close={close} commands={commands} />
              <CommandMenuItem id="camera-front" close={close} commands={commands} />
              <CommandMenuItem id="camera-right" close={close} commands={commands} />
              <CommandMenuItem id="camera-fit" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="toggle-wireframe" close={close} commands={commands} />
              <CommandMenuItem id="toggle-grid-snap" close={close} commands={commands} />
              <MenuSeparator />
              <RayTracingSubmenu />
              <MouseControlsSubmenu />
              <MenuSeparator />
              <CommandMenuItem id="cycle-theme" close={close} commands={commands} />
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="Tools" accelerator="T">
          {(close: () => void) => (
            <>
              <CommandMenuItem id="command-palette" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="new-sketch" close={close} commands={commands} />
              <MenuSeparator />
              <CommandMenuItem id="open-slicer" close={close} commands={commands} />
              <CommandMenuItem id="open-cam" close={close} commands={commands} />
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="Help" accelerator="H">
          {(close: () => void) => (
            <>
              <CommandMenuItem id="about" close={close} commands={commands} />
              <CommandMenuItem
                id="whats-new"
                close={close}
                commands={commands}
                badge={unreadChangelog}
              />
              <MenuSeparator />
              <CommandMenuItem id="open-docs" close={close} commands={commands} />
              <CommandMenuItem id="open-github" close={close} commands={commands} />
              <CommandMenuItem id="open-discord" close={close} commands={commands} />
            </>
          )}
        </MenuBarItem>

        <div className="flex-1" />

        {/* Right cluster: auth */}
        <SignInButton
          variant="icon-text"
          className={cn(
            "flex items-center justify-center gap-1.5 h-6 px-2 text-[10px] font-medium",
            "text-text-muted hover:text-text hover:bg-hover",
          )}
        />
        <UserMenu onSyncNow={() => triggerSync()} />
      </div>

      {/* ─────────────────────────────────────────────────────── */}
      {/* Row 2+: tool palette (tab strip + icon row) docked under */}
      {/* ─────────────────────────────────────────────────────── */}
      {children}
    </div>
  );
}
