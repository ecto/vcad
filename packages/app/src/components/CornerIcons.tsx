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
import { Keyboard } from "@phosphor-icons/react/dist/ssr/Keyboard";
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
import * as Popover from "@radix-ui/react-popover";
import { Tooltip } from "@/components/ui/tooltip";
import {
  useDocumentStore,
  useUiStore,
} from "@vcad/core";
import { cn } from "@/lib/utils";
import { examples } from "@/data/examples";
import { CameraSettingsPanel } from "./CameraSettingsPanel";
import { useCameraSettingsStore } from "@/stores/camera-settings-store";
import { CONTROL_PRESETS } from "@/types/camera-controls";
import { SignInButton, UserMenu, triggerSync } from "@vcad/auth";
import { useBackgroundLuminance } from "@/hooks/useBackgroundLuminance";
import { useChangelogStore } from "@/stores/changelog-store";

interface CornerIconsProps {
  onAboutOpen: () => void;
  onSave: () => void;
  onOpen: () => void;
}

function IconButton({
  children,
  onClick,
  tooltip,
  active,
  className,
}: {
  children: React.ReactNode;
  onClick?: () => void;
  tooltip: string;
  active?: boolean;
  className?: string;
}) {
  return (
    <Tooltip content={tooltip}>
      <button
        className={cn(
          // Mobile: 44px touch targets; Desktop: 32px
          "flex h-11 w-11 sm:h-8 sm:w-8 items-center justify-center",
          "text-text-muted/70 hover:text-text hover:bg-hover",
          active && "text-accent",
          className,
        )}
        onClick={onClick}
      >
        {children}
      </button>
    </Tooltip>
  );
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
              href="https://docs.rs/vcad"
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

export function CornerIcons({ onAboutOpen, onSave, onOpen }: CornerIconsProps) {
  const isDirty = useDocumentStore((s) => s.isDirty);
  const toggleFeatureTree = useUiStore((s) => s.toggleFeatureTree);
  const featureTreeOpen = useUiStore((s) => s.featureTreeOpen);
  const isOrbiting = useUiStore((s) => s.isOrbiting);

  // Adaptive text based on background luminance
  const bgLuminance = useBackgroundLuminance();
  const logoColor = bgLuminance === "light" ? "text-gray-900" : "text-white";

  return (
    <>
      {/* Top-left: hamburger + logo - with safe area padding */}
      <div className={cn(
        "absolute z-20 flex items-center gap-2 top-[max(0.75rem,var(--safe-top))] left-[max(0.75rem,var(--safe-left))]",
        "transition-all duration-300",
        isOrbiting && "opacity-0 pointer-events-none"
      )}>
        <IconButton
          tooltip="Toggle sidebar (`)"
          onClick={toggleFeatureTree}
          active={featureTreeOpen}
        >
          <List size={20} />
        </IconButton>
        <div className="flex items-center gap-1 pl-1">
          <span className={cn("text-sm font-bold tracking-tighter transition-colors duration-300", logoColor)}>
            vcad<span className="text-accent">.</span>
          </span>
          {isDirty && <span className="text-accent text-xs">*</span>}
        </div>
      </div>

      {/* Top-right: save, settings, auth - with safe area padding */}
      <div className={cn(
        "absolute z-20 flex items-center gap-1 top-[max(0.75rem,var(--safe-top))] right-[max(0.75rem,var(--safe-right))]",
        "transition-opacity duration-200",
        isOrbiting && "opacity-0 pointer-events-none"
      )}>
        {/* Save - always visible */}
        <IconButton tooltip="Save (Cmd+S)" onClick={onSave}>
          <FloppyDisk size={18} />
        </IconButton>

        {/* Settings menu (includes Open, Theme, Cmd+K, GitHub, Discord) */}
        <SettingsMenu onAboutOpen={onAboutOpen} onOpen={onOpen} />

        {/* Auth: Sign in button or user menu */}
        <SignInButton
          variant="icon-text"
          className={cn(
            "flex items-center justify-center gap-1.5 h-11 w-11 sm:h-8 sm:w-auto sm:px-2 text-xs font-medium",
            "text-text-muted hover:text-text hover:bg-hover rounded",
          )}
        />
        <UserMenu onSyncNow={() => triggerSync()} />
      </div>
    </>
  );
}
