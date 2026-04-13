import { useState } from "react";
import { Sun } from "@phosphor-icons/react/dist/ssr/Sun";
import { Moon } from "@phosphor-icons/react/dist/ssr/Moon";
import { Desktop } from "@phosphor-icons/react/dist/ssr/Desktop";
import { Command } from "@phosphor-icons/react/dist/ssr/Command";
import { List } from "@phosphor-icons/react/dist/ssr/List";
import { CubeTransparent } from "@phosphor-icons/react/dist/ssr/CubeTransparent";
import { GridFour } from "@phosphor-icons/react/dist/ssr/GridFour";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { GithubLogo } from "@phosphor-icons/react/dist/ssr/GithubLogo";
import { DiscordLogo } from "@phosphor-icons/react/dist/ssr/DiscordLogo";
import { Mouse } from "@phosphor-icons/react/dist/ssr/Mouse";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { Rocket } from "@phosphor-icons/react/dist/ssr/Rocket";
import { FloppyDisk } from "@phosphor-icons/react/dist/ssr/FloppyDisk";
import { ChatDots } from "@phosphor-icons/react/dist/ssr/ChatDots";
import { FilePlus } from "@phosphor-icons/react/dist/ssr/FilePlus";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Files } from "@phosphor-icons/react/dist/ssr/Files";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";
import { ArrowClockwise } from "@phosphor-icons/react/dist/ssr/ArrowClockwise";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { ClipboardText } from "@phosphor-icons/react/dist/ssr/ClipboardText";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Selection } from "@phosphor-icons/react/dist/ssr/Selection";
import { Pencil } from "@phosphor-icons/react/dist/ssr/Pencil";
import { Terminal } from "@phosphor-icons/react/dist/ssr/Terminal";
import { Printer } from "@phosphor-icons/react/dist/ssr/Printer";
import { Wrench } from "@phosphor-icons/react/dist/ssr/Wrench";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import * as Popover from "@radix-ui/react-popover";
import {
  useDocumentStore,
  useUiStore,
  useChatStore,
  useEngineStore,
  useSketchStore,
  exportStlBlob,
  exportGltfBlob,
  exportStepBlob,
} from "@vcad/core";
import { cn } from "@/lib/utils";
import { downloadBlob } from "@/lib/download";
import { examples } from "@/data/examples";
import { CameraSettingsPanel } from "./CameraSettingsPanel";
import { useCameraSettingsStore } from "@/stores/camera-settings-store";
import { CONTROL_PRESETS } from "@/types/camera-controls";
import { SignInButton, UserMenu, triggerSync } from "@vcad/auth";
import { useChangelogStore } from "@/stores/changelog-store";
import { useLogStore } from "@/stores/log-store";
import { useSlicerStore } from "@/stores/slicer-store";
import { useCamStore } from "@/stores/cam-store";
import { useNotificationStore } from "@/stores/notification-store";

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
}: {
  onClick: () => void;
  children: React.ReactNode;
  shortcut?: string;
  icon?: React.ComponentType<{ size?: number; className?: string }>;
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover"
    >
      {Icon && <Icon size={13} className="text-text-muted" />}
      <span className="flex-1 text-left">{children}</span>
      {shortcut && <span className="text-text-muted text-[10px]">{shortcut}</span>}
    </button>
  );
}

function MenuSeparator() {
  return <div className="my-1 border-t border-border" />;
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
          <Sparkle size={13} className={renderMode === "raytrace" ? "text-accent" : "text-text-muted"} />
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
            <span className={renderMode === "standard" ? "text-accent" : "text-text"}>Off</span>
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
                  renderMode === "raytrace" && raytraceQuality === q ? "text-accent" : "text-text"
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
            <span className={raytraceEdgesEnabled ? "text-accent" : "text-text"}>
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
  const toggleFeatureTree = useUiStore((s) => s.toggleFeatureTree);
  const setTheme = useUiStore((s) => s.setTheme);
  const theme = useUiStore((s) => s.theme);
  const toggleWireframe = useUiStore((s) => s.toggleWireframe);
  const showWireframe = useUiStore((s) => s.showWireframe);
  const toggleGridSnap = useUiStore((s) => s.toggleGridSnap);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const unreadChangelog = useChangelogStore((s) => s.getUnreadCount());

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

  const handleNew = () => {
    if (useDocumentStore.getState().isDirty) {
      if (!window.confirm("Discard unsaved changes and start a new document?")) return;
    }
    useDocumentStore.getState().newDocument(crypto.randomUUID(), "Untitled");
  };

  const handleUndo = () => useDocumentStore.getState().undo();
  const handleRedo = () => useDocumentStore.getState().redo();

  const handleDelete = () => {
    const { selectedPartIds, clearSelection } = useUiStore.getState();
    if (selectedPartIds.size === 0) return;
    const { removePart } = useDocumentStore.getState();
    for (const id of selectedPartIds) removePart(id);
    clearSelection();
  };

  const handleDuplicate = () => {
    const { selectedPartIds, selectMultiple } = useUiStore.getState();
    if (selectedPartIds.size === 0) return;
    const newIds = useDocumentStore.getState().duplicateParts(Array.from(selectedPartIds));
    selectMultiple(newIds);
  };

  const handleCopy = () => {
    const { selectedPartIds, copyToClipboard } = useUiStore.getState();
    if (selectedPartIds.size === 0) return;
    copyToClipboard(Array.from(selectedPartIds));
    useNotificationStore
      .getState()
      .addToast(`Copied ${selectedPartIds.size} part${selectedPartIds.size > 1 ? "s" : ""}`, "success");
  };

  const handlePaste = () => {
    const { clipboard, selectMultiple } = useUiStore.getState();
    if (clipboard.length === 0) return;
    const newIds = useDocumentStore.getState().duplicateParts(clipboard);
    selectMultiple(newIds);
  };

  const handleSelectAll = () => {
    const parts = useDocumentStore.getState().parts;
    useUiStore.getState().selectMultiple(parts.map((p) => p.id));
  };

  const handleLoadExample = (file: unknown) => {
    window.dispatchEvent(new CustomEvent("vcad:load-example", { detail: { file } }));
  };

  const handleCameraPreset = (preset: string) => {
    window.dispatchEvent(new CustomEvent(`vcad:camera-${preset}`));
  };

  const handleStartSketch = () => {
    useSketchStore.getState().enterFaceSelectionMode();
    useNotificationStore.getState().addToast("Select a face to sketch on", "info");
  };

  return (
    <div className="flex flex-col bg-surface">
      {/* ─────────────────────────────────────────────────────── */}
      {/* Row 1: menu bar (logo + File/Edit/View/Tools/Help)     */}
      {/* ─────────────────────────────────────────────────────── */}
      <div className="relative flex h-7 items-center gap-0 px-2 border-b border-border/30">
        <div className="flex items-center gap-1 pr-3">
          <span className="text-sm font-bold tracking-tighter text-text">
            vcad<span className="text-accent">.</span>
          </span>
          {isDirty && <span className="text-accent text-xs">*</span>}
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
              <MenuItem icon={FilePlus} shortcut="⌘N" onClick={() => { handleNew(); close(); }}>
                New
              </MenuItem>
              <MenuItem icon={FolderOpen} shortcut="⌘O" onClick={() => { onOpen(); close(); }}>
                Open…
              </MenuItem>
              <MenuItem
                icon={Files}
                shortcut="⌘⇧O"
                onClick={() => { window.dispatchEvent(new CustomEvent("vcad:documents")); close(); }}
              >
                Open from Cloud…
              </MenuItem>
              <MenuSeparator />
              <MenuItem icon={FloppyDisk} shortcut="⌘S" onClick={() => { onSave(); close(); }}>
                Save
              </MenuItem>
              <MenuSeparator />
              <Popover.Root>
                <Popover.Trigger asChild>
                  <button className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover">
                    <Export size={13} className="text-text-muted" />
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
                    <BookOpen size={13} className="text-text-muted" />
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
              <MenuItem icon={ArrowCounterClockwise} shortcut="⌘Z" onClick={() => { handleUndo(); close(); }}>
                Undo
              </MenuItem>
              <MenuItem icon={ArrowClockwise} shortcut="⌘⇧Z" onClick={() => { handleRedo(); close(); }}>
                Redo
              </MenuItem>
              <MenuSeparator />
              <MenuItem icon={Copy} shortcut="⌘C" onClick={() => { handleCopy(); close(); }}>
                Copy
              </MenuItem>
              <MenuItem icon={ClipboardText} shortcut="⌘V" onClick={() => { handlePaste(); close(); }}>
                Paste
              </MenuItem>
              <MenuItem icon={Copy} shortcut="⌘D" onClick={() => { handleDuplicate(); close(); }}>
                Duplicate
              </MenuItem>
              <MenuItem icon={Trash} shortcut="Del" onClick={() => { handleDelete(); close(); }}>
                Delete
              </MenuItem>
              <MenuSeparator />
              <MenuItem icon={Selection} shortcut="⌘A" onClick={() => { handleSelectAll(); close(); }}>
                Select All
              </MenuItem>
              <MenuItem
                shortcut="Esc"
                onClick={() => { useUiStore.getState().clearSelection(); close(); }}
              >
                Deselect
              </MenuItem>
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="View" accelerator="V">
          {(close: () => void) => (
            <>
              <MenuItem
                icon={List}
                onClick={() => { toggleFeatureTree(); close(); }}
              >
                Toggle Left Sidebar
              </MenuItem>
              <MenuItem
                icon={ChatDots}
                shortcut="F6"
                onClick={() => { useChatStore.getState().toggleOpen(); close(); }}
              >
                Toggle Right Sidebar
              </MenuItem>
              <MenuItem
                icon={Terminal}
                onClick={() => { useUiStore.getState().toggleStatusBar(); close(); }}
              >
                Toggle Status Bar
              </MenuItem>
              <MenuItem
                icon={Terminal}
                shortcut="`"
                onClick={() => { useLogStore.getState().togglePanel(); close(); }}
              >
                Toggle DevTools
              </MenuItem>
              <MenuSeparator />
              <MenuItem icon={Cube} onClick={() => { handleCameraPreset("isometric"); close(); }}>
                Isometric
              </MenuItem>
              <MenuItem onClick={() => { handleCameraPreset("top"); close(); }}>Top</MenuItem>
              <MenuItem onClick={() => { handleCameraPreset("front"); close(); }}>Front</MenuItem>
              <MenuItem onClick={() => { handleCameraPreset("right"); close(); }}>Right</MenuItem>
              <MenuItem
                icon={ArrowsOutCardinal}
                shortcut="F"
                onClick={() => { handleCameraPreset("fit"); close(); }}
              >
                Fit to View
              </MenuItem>
              <MenuSeparator />
              <MenuItem
                icon={CubeTransparent}
                shortcut="X"
                onClick={() => { toggleWireframe(); close(); }}
              >
                {showWireframe ? "Hide Wireframe" : "Show Wireframe"}
              </MenuItem>
              <MenuItem
                icon={GridFour}
                shortcut="G"
                onClick={() => { toggleGridSnap(); close(); }}
              >
                {gridSnap ? "Disable Grid Snap" : "Enable Grid Snap"}
              </MenuItem>
              <MenuSeparator />
              <RayTracingSubmenu />
              <MouseControlsSubmenu />
              <MenuSeparator />
              <MenuItem
                icon={theme === "dark" ? Sun : theme === "light" ? Desktop : Moon}
                onClick={() => {
                  setTheme(theme === "dark" ? "light" : theme === "light" ? "system" : "dark");
                  close();
                }}
              >
                {theme === "dark" ? "Light Theme" : theme === "light" ? "System Theme" : "Dark Theme"}
              </MenuItem>
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="Tools" accelerator="T">
          {(close: () => void) => (
            <>
              <MenuItem icon={Command} shortcut="⌘K" onClick={() => { handleCommandPalette(); close(); }}>
                Command Palette…
              </MenuItem>
              <MenuSeparator />
              <MenuItem icon={Pencil} onClick={() => { handleStartSketch(); close(); }}>
                New Sketch…
              </MenuItem>
              <MenuSeparator />
              <MenuItem
                icon={Printer}
                onClick={() => { useSlicerStore.getState().openPrintPanel(); close(); }}
              >
                Print (Slicer)…
              </MenuItem>
              <MenuItem
                icon={Wrench}
                onClick={() => { useCamStore.getState().openCamPanel(); close(); }}
              >
                CAM (Toolpath)…
              </MenuItem>
            </>
          )}
        </MenuBarItem>

        <MenuBarItem label="Help" accelerator="H">
          {(close: () => void) => (
            <>
              <MenuItem icon={Info} onClick={() => { onAboutOpen(); close(); }}>
                About vcad
              </MenuItem>
              <button
                onClick={() => {
                  useChangelogStore.getState().openPanel();
                  close();
                }}
                className="flex w-full items-center gap-2 px-3 py-1 text-xs text-text hover:bg-hover"
              >
                <Rocket size={13} className="text-accent" />
                <span className="flex-1 text-left">What's New</span>
                {unreadChangelog > 0 && (
                  <span className="px-1.5 py-0.5 text-[10px] font-bold bg-accent text-white rounded-full min-w-[18px] text-center">
                    {unreadChangelog}
                  </span>
                )}
              </button>
              <MenuSeparator />
              <MenuItem
                icon={BookOpen}
                onClick={() => { window.open("https://docs.vcad.io", "_blank"); close(); }}
              >
                Documentation
              </MenuItem>
              <MenuItem
                icon={GithubLogo}
                onClick={() => { window.open("https://github.com/ecto/vcad", "_blank"); close(); }}
              >
                GitHub
              </MenuItem>
              <MenuItem
                icon={DiscordLogo}
                onClick={() => { window.open("https://discord.gg/ZU8QHnFAc2", "_blank"); close(); }}
              >
                Discord
              </MenuItem>
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
