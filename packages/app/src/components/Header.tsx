import { useState, useEffect } from "react";
import { Sun } from "@phosphor-icons/react/dist/ssr/Sun";
import { Moon } from "@phosphor-icons/react/dist/ssr/Moon";
import { Desktop } from "@phosphor-icons/react/dist/ssr/Desktop";
import { Command } from "@phosphor-icons/react/dist/ssr/Command";
import { DotsThree } from "@phosphor-icons/react/dist/ssr/DotsThree";
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
import { Keyboard } from "@phosphor-icons/react/dist/ssr/Keyboard";
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

function ViewButton({
  children,
  onClick,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title: string;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      className="flex h-7 w-10 items-center justify-center text-[10px] font-medium text-text hover:bg-hover border border-border"
    >
      {children}
    </button>
  );
}


function WhatsNewMenuItem({ onClose }: { onClose: () => void }) {
  const openPanel = useChangelogStore((s) => s.openPanel);
  const getUnreadCount = useChangelogStore((s) => s.getUnreadCount);
  const unreadCount = getUnreadCount();

  return (
    <button
      onClick={() => {
        openPanel();
        onClose();
      }}
      className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
    >
      <Rocket size={14} className="text-accent" />
      <span>What's New</span>
      {unreadCount > 0 && (
        <span className="ml-auto px-1.5 py-0.5 text-[10px] font-bold bg-accent text-white rounded-full min-w-[18px] text-center">
          {unreadCount}
        </span>
      )}
      {unreadCount === 0 && (
        <span className="ml-auto text-text-muted text-[10px]">?</span>
      )}
    </button>
  );
}

function SettingsMenu({ onAboutOpen, onOpen }: { onAboutOpen: () => void; onOpen: () => void }) {
  const [open, setOpen] = useState(false);
  const [showAllExamples, setShowAllExamples] = useState(false);

  // Close on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && open) {
        setOpen(false);
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open]);

  const showWireframe = useUiStore((s) => s.showWireframe);
  const toggleWireframe = useUiStore((s) => s.toggleWireframe);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const toggleGridSnap = useUiStore((s) => s.toggleGridSnap);
  const snapIncrement = useUiStore((s) => s.snapIncrement);
  const setSnapIncrement = useUiStore((s) => s.setSnapIncrement);
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceQuality = useUiStore((s) => s.raytraceQuality);
  const raytraceDebugMode = useUiStore((s) => s.raytraceDebugMode);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const raytraceEdgesEnabled = useUiStore((s) => s.raytraceEdgesEnabled);
  const toggleRenderMode = useUiStore((s) => s.toggleRenderMode);
  const setRaytraceQuality = useUiStore((s) => s.setRaytraceQuality);
  const setRaytraceDebugMode = useUiStore((s) => s.setRaytraceDebugMode);
  const setRaytraceEdgesEnabled = useUiStore((s) => s.setRaytraceEdgesEnabled);
  const theme = useUiStore((s) => s.theme);
  const toggleTheme = useUiStore((s) => s.toggleTheme);

  // Camera settings
  const controlSchemeId = useCameraSettingsStore((s) => s.controlSchemeId);
  const currentSchemeName = CONTROL_PRESETS[controlSchemeId]?.name ?? "vcad";

  // Featured examples shown by default
  const featuredExamples = examples.slice(0, 3);
  const remainingExamples = examples.slice(3);
  const displayedExamples = showAllExamples ? examples : featuredExamples;

  function handleLoadExample(exampleId: string) {
    const example = examples.find((e) => e.id === exampleId);
    if (example) {
      window.dispatchEvent(
        new CustomEvent("vcad:load-example", { detail: { file: example.file } }),
      );
    }
  }

  function handleCameraPreset(preset: string) {
    window.dispatchEvent(new CustomEvent(`vcad:camera-${preset}`));
  }

  const themeLabel = theme === "system" ? "System" : theme === "light" ? "Light" : "Dark";
  const ThemeIcon = theme === "system" ? Desktop : theme === "light" ? Sun : Moon;

  // Changelog badge
  const getUnreadCount = useChangelogStore((s) => s.getUnreadCount);
  const unreadCount = getUnreadCount();

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          className={cn(
            "relative flex h-8 w-8 items-center justify-center",
            "text-text-muted/70 hover:text-text hover:bg-hover",
          )}
        >
          <DotsThree size={20} weight="bold" />
          {unreadCount > 0 && (
            <span className="absolute -top-0.5 -right-0.5 w-4 h-4 flex items-center justify-center text-[9px] font-bold bg-accent text-white rounded-full">
              {unreadCount > 9 ? "9+" : unreadCount}
            </span>
          )}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className="z-50 w-56 border border-border bg-surface p-2 shadow-xl max-h-[80vh] overflow-y-auto"
          sideOffset={8}
          align="end"
        >
          {/* File Section */}
          <div className="px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-text-muted">
            File
          </div>
          <button
            onClick={() => {
              onOpen();
              setOpen(false);
            }}
            className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
          >
            <FolderOpen size={14} />
            <span>Open File</span>
            <span className="ml-auto text-text-muted text-[10px]">⌘O</span>
          </button>
          <button
            onClick={() => {
              window.dispatchEvent(new CustomEvent("vcad:open-chat"));
              setOpen(false);
            }}
            className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
          >
            <Command size={14} />
            <span>Chat</span>
            <span className="ml-auto text-text-muted text-[10px]">⌘K</span>
          </button>

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* What's New */}
          <WhatsNewMenuItem onClose={() => setOpen(false)} />

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* Appearance Section */}
          <div className="px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-text-muted">
            Appearance
          </div>

          {/* Theme submenu */}
          <Popover.Root>
            <Popover.Trigger asChild>
              <button className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover">
                <ThemeIcon size={14} />
                <span>Theme</span>
                <span className="ml-auto text-text-muted">
                  {themeLabel} <CaretRight size={10} className="inline" />
                </span>
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                className="z-50 border border-border bg-surface p-1.5 shadow-xl"
                side="right"
                sideOffset={4}
                align="start"
              >
                <div className="flex flex-col gap-0.5">
                  <button
                    onClick={() => {
                      while (useUiStore.getState().theme !== "system") toggleTheme();
                    }}
                    className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                  >
                    <Desktop size={14} />
                    <span className={theme === "system" ? "text-accent" : ""}>System</span>
                  </button>
                  <button
                    onClick={() => {
                      while (useUiStore.getState().theme !== "light") toggleTheme();
                    }}
                    className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                  >
                    <Sun size={14} />
                    <span className={theme === "light" ? "text-accent" : ""}>Light</span>
                  </button>
                  <button
                    onClick={() => {
                      while (useUiStore.getState().theme !== "dark") toggleTheme();
                    }}
                    className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                  >
                    <Moon size={14} />
                    <span className={theme === "dark" ? "text-accent" : ""}>Dark</span>
                  </button>
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* Examples Section */}
          <div className="px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-text-muted">
            Try an Example
          </div>
          <div className="flex flex-wrap gap-1 px-1 py-1">
            {displayedExamples.map((example) => (
              <button
                key={example.id}
                onClick={() => handleLoadExample(example.id)}
                className="px-2 py-1 text-xs text-text hover:bg-hover border border-border"
              >
                {example.name}
              </button>
            ))}
          </div>
          {!showAllExamples && remainingExamples.length > 0 && (
            <button
              onClick={() => setShowAllExamples(true)}
              className="w-full px-2 py-1 text-[10px] text-text-muted hover:text-text"
            >
              + {remainingExamples.length} more...
            </button>
          )}

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* View Section */}
          <div className="px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-text-muted">
            View
          </div>
          <div className="flex gap-1 px-1 py-1">
            <ViewButton
              onClick={() => handleCameraPreset("isometric")}
              title="Isometric view"
            >
              <Cube size={14} />
            </ViewButton>
            <ViewButton
              onClick={() => handleCameraPreset("fit")}
              title="Fit all in view"
            >
              <ArrowsOutCardinal size={14} />
            </ViewButton>
          </div>
          <div className="flex gap-1 px-1 py-1">
            <ViewButton
              onClick={() => handleCameraPreset("top")}
              title="Top view (looking down Z)"
            >
              Top
            </ViewButton>
            <ViewButton
              onClick={() => handleCameraPreset("front")}
              title="Front view (looking down Y)"
            >
              Front
            </ViewButton>
            <ViewButton
              onClick={() => handleCameraPreset("right")}
              title="Right view (looking down X)"
            >
              Right
            </ViewButton>
          </div>

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* Wireframe toggle */}
          <button
            onClick={toggleWireframe}
            className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
          >
            <CubeTransparent
              size={14}
              className={showWireframe ? "text-accent" : ""}
            />
            <span>Wireframe</span>
            <span className="ml-auto text-text-muted">X</span>
          </button>

          {/* Ray Tracing toggle with quality submenu */}
          {raytraceAvailable && (
            <Popover.Root>
              <Popover.Trigger asChild>
                <button className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover">
                  <Sparkle
                    size={14}
                    className={renderMode === "raytrace" ? "text-accent" : ""}
                  />
                  <span>Ray Tracing</span>
                  <span className="ml-auto text-text-muted">
                    {renderMode === "raytrace" ? raytraceQuality : "Off"} &rsaquo;
                  </span>
                </button>
              </Popover.Trigger>
              <Popover.Portal>
                <Popover.Content
                  className="z-50 border border-border bg-surface p-1.5 shadow-xl"
                  side="right"
                  sideOffset={4}
                  align="start"
                >
                  <div className="flex flex-col gap-0.5">
                    <button
                      onClick={toggleRenderMode}
                      className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                    >
                      <span className={renderMode === "standard" ? "text-accent" : ""}>Off</span>
                    </button>
                    {(["draft", "standard", "high"] as const).map((q) => (
                      <button
                        key={q}
                        onClick={() => {
                          if (renderMode !== "raytrace") {
                            toggleRenderMode();
                          }
                          setRaytraceQuality(q);
                        }}
                        className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                      >
                        <span
                          className={
                            renderMode === "raytrace" && raytraceQuality === q ? "text-accent" : ""
                          }
                        >
                          {q.charAt(0).toUpperCase() + q.slice(1)}
                          {q === "draft" && " (0.5x)"}
                          {q === "standard" && " (1x)"}
                          {q === "high" && " (2x)"}
                        </span>
                      </button>
                    ))}

                    {/* Debug modes separator */}
                    {renderMode === "raytrace" && (
                      <>
                        <div className="my-1 border-t border-border" />
                        <div className="px-2 py-0.5 text-[9px] font-bold uppercase tracking-wider text-text-muted">
                          Debug
                        </div>
                        {([
                          ["off", "Off"],
                          ["normals", "Normals"],
                          ["face-id", "Face ID"],
                          ["lighting", "N·L"],
                          ["orientation", "Orientation"],
                        ] as const).map(([mode, label]) => (
                          <button
                            key={mode}
                            onClick={() => setRaytraceDebugMode(mode)}
                            className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                          >
                            <span className={raytraceDebugMode === mode ? "text-accent" : ""}>
                              {label}
                            </span>
                          </button>
                        ))}
                        {/* Edge detection toggle */}
                        <div className="my-1 border-t border-border" />
                        <button
                          onClick={() => setRaytraceEdgesEnabled(!raytraceEdgesEnabled)}
                          className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                        >
                          <span className={raytraceEdgesEnabled ? "text-accent" : ""}>
                            Edges {raytraceEdgesEnabled ? "On" : "Off"}
                          </span>
                        </button>
                      </>
                    )}
                  </div>
                </Popover.Content>
              </Popover.Portal>
            </Popover.Root>
          )}

          {/* Grid Snap with submenu */}
          <Popover.Root>
            <Popover.Trigger asChild>
              <button className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover">
                <GridFour
                  size={14}
                  className={gridSnap ? "text-accent" : ""}
                />
                <span>Grid Snap</span>
                <span className="ml-auto text-text-muted">
                  {gridSnap ? `${snapIncrement}mm` : "Off"} &rsaquo;
                </span>
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                className="z-50 border border-border bg-surface p-1.5 shadow-xl"
                side="right"
                sideOffset={4}
                align="start"
              >
                <div className="flex flex-col gap-0.5">
                  <button
                    onClick={toggleGridSnap}
                    className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                  >
                    <span className={!gridSnap ? "text-accent" : ""}>Off</span>
                  </button>
                  {[1, 2, 5, 10, 25, 50].map((v) => (
                    <button
                      key={v}
                      onClick={() => setSnapIncrement(v)}
                      className="flex items-center gap-2 px-2 py-1 text-xs text-text hover:bg-hover"
                    >
                      <span
                        className={
                          gridSnap && snapIncrement === v ? "text-accent" : ""
                        }
                      >
                        {v}mm
                      </span>
                    </button>
                  ))}
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>

          {/* Camera Controls with submenu */}
          <Popover.Root>
            <Popover.Trigger asChild>
              <button className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover">
                <Mouse size={14} />
                <span>Controls</span>
                <span className="ml-auto text-text-muted">
                  {currentSchemeName} &rsaquo;
                </span>
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                className="z-50 w-64 border border-border bg-surface p-2 shadow-xl max-h-[80vh] overflow-y-auto"
                side="right"
                sideOffset={4}
                align="start"
              >
                <CameraSettingsPanel />
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* Help Section */}
          <div className="flex gap-1 px-1 py-1">
            <button
              onClick={onAboutOpen}
              className="flex flex-1 items-center justify-center gap-1.5 px-2 py-1.5 text-xs text-text hover:bg-hover"
              title="Keyboard shortcuts"
            >
              <Keyboard size={14} />
              Shortcuts
            </button>
            <a
              href="https://docs.vcad.io"
              target="_blank"
              rel="noopener noreferrer"
              className="flex flex-1 items-center justify-center gap-1.5 px-2 py-1.5 text-xs text-text hover:bg-hover"
              title="Documentation"
            >
              <BookOpen size={14} />
              Docs
            </a>
          </div>
          <button
            onClick={onAboutOpen}
            className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
          >
            <Info size={14} />
            <span>About</span>
          </button>

          {/* Divider */}
          <div className="my-2 border-t border-border" />

          {/* External Links Section */}
          <div className="px-2 py-1 text-[10px] font-bold uppercase tracking-wider text-text-muted">
            Links
          </div>
          <a
            href="https://github.com/ecto/vcad"
            target="_blank"
            rel="noopener noreferrer"
            className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
          >
            <GithubLogo size={14} />
            <span>GitHub</span>
          </a>
          <a
            href="https://discord.gg/ZU8QHnFAc2"
            target="_blank"
            rel="noopener noreferrer"
            className="flex w-full items-center gap-2 px-2 py-1.5 text-xs text-text hover:bg-hover"
          >
            <DiscordLogo size={14} />
            <span>Discord</span>
          </a>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
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

export function Header({ onAboutOpen, onSave, onOpen, children }: HeaderProps) {
  const isDirty = useDocumentStore((s) => s.isDirty);
  const toggleFeatureTree = useUiStore((s) => s.toggleFeatureTree);
  const featureTreeOpen = useUiStore((s) => s.featureTreeOpen);
  const setTheme = useUiStore((s) => s.setTheme);
  const theme = useUiStore((s) => s.theme);
  const chatOpen = useChatStore((s) => s.open);
  const toggleWireframe = useUiStore((s) => s.toggleWireframe);
  const showWireframe = useUiStore((s) => s.showWireframe);
  const toggleGridSnap = useUiStore((s) => s.toggleGridSnap);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const logPanelOpen = useLogStore((s) => s.panelOpen);
  const unreadChangelog = useChangelogStore((s) => s.getUnreadCount());

  const handleOpenChat = () => {
    useChatStore.getState().setOpen(true);
    window.dispatchEvent(new CustomEvent("vcad:open-chat"));
  };
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
      <div className="flex h-6 items-center gap-0 px-2 border-b border-border/30">
        <div className="flex items-center gap-1 pr-3">
          <span className="text-sm font-bold tracking-tighter text-text">
            vcad<span className="text-accent">.</span>
          </span>
          {isDirty && <span className="text-accent text-xs">*</span>}
        </div>

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
                {featureTreeOpen ? "Hide Feature Tree" : "Show Feature Tree"}
              </MenuItem>
              <MenuItem
                icon={ChatDots}
                shortcut="F6"
                onClick={() => { useChatStore.getState().toggleOpen(); close(); }}
              >
                {chatOpen ? "Hide Chat" : "Show Chat"}
              </MenuItem>
              <MenuItem
                icon={Terminal}
                shortcut="`"
                onClick={() => { useLogStore.getState().togglePanel(); close(); }}
              >
                {logPanelOpen ? "Hide Log Viewer" : "Show Log Viewer"}
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
              <MenuItem icon={ChatDots} onClick={() => { handleOpenChat(); close(); }}>
                AI Chat
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

        {/* Right cluster: viewport settings + auth */}
        <SettingsMenu onAboutOpen={onAboutOpen} onOpen={onOpen} />
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
