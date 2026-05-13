import { lazy, Suspense, useEffect, useState } from "react";
import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { Mouse } from "@phosphor-icons/react/dist/ssr/Mouse";
import { Goggles } from "@phosphor-icons/react/dist/ssr/Goggles";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { Bell } from "@phosphor-icons/react/dist/ssr/Bell";
import { Megaphone } from "@phosphor-icons/react/dist/ssr/Megaphone";
import { Link as LinkIcon } from "@phosphor-icons/react/dist/ssr/Link";
import { DocTitle } from "@/components/DocTitle";
import { Tooltip } from "@/components/ui/tooltip";
import * as Menubar from "@radix-ui/react-menubar";
import {
  useDocumentStore,
  useUiStore,
  useEngineStore,
  exportStlBlob,
  exportGltfBlob,
  exportStepBlob,
  CATEGORY_ICON_COLORS,
  TIERS,
  useBillingStore,
  t,
  runJob,
  type Command,
} from "@vcad/core";
import { openCustomerPortal } from "@/lib/billing-api";
import { UpgradeModal } from "@/components/UpgradeModal";
import { cn } from "@/lib/utils";
import { downloadBlob } from "@/lib/download";
import { examples } from "@/data/examples";
import { SignInButton, UserMenu, triggerSync, useAuthStore } from "@vcad/auth";
import { useChangelogStore } from "@/stores/changelog-store";
import { useNotificationStore } from "@/stores/notification-store";
import { useDfmStore } from "@/stores/dfm-store";
import { useAppCommands } from "@/hooks/useAppCommands";
import { COMMAND_ICONS } from "@/lib/command-icons";
import { useCapabilities } from "@/lib/capabilities";
import { useNativeMenu } from "@/hooks/useNativeMenu";
import { xrStore, useXRPresenting, useXRSupportStore } from "@/stores/xr-store";

// Lazy so the prefs dialog (and its wasm-backed keybinding hook) doesn't
// bloat the Header bundle until the user opens it.
const InputPreferencesDialog = lazy(() =>
  import("./InputPreferencesDialog").then((m) => ({
    default: m.InputPreferencesDialog,
  })),
);

const FeedbackModal = lazy(() =>
  import("./FeedbackModal").then((m) => ({ default: m.FeedbackModal })),
);

interface HeaderProps {
  onAboutOpen: () => void;
  onProductOpen: () => void;
  onSave: () => void;
  onOpen: () => void;
  /** Opens the ShareDialog. Enabled only when signed in + cloud-synced. */
  onShareOpen: () => void;
  /** Opens the VersionHistoryModal. Enabled only when signed in. */
  onVersionHistoryOpen: () => void;
  /** Tool palette (tab strip + icon row) docked directly under the menu bar. */
  children?: React.ReactNode;
}


// ---------------------------------------------------------------------------
// Borland C++ Builder / Delphi-style menu bar — built on Radix Menubar for
// native keyboard nav: Tab/Arrow between menus, typeahead inside them,
// hover-handoff between triggers and submenus.
// ---------------------------------------------------------------------------

const TRIGGER_CLASS = cn(
  "h-6 px-2 text-[11px] font-medium text-text outline-none cursor-default select-none",
  "hover:bg-hover data-[state=open]:bg-hover",
);

const CONTENT_CLASS =
  "z-50 min-w-[200px] rounded-lg border border-border/80 bg-surface p-1 shadow-md";

const ITEM_CLASS = cn(
  "flex w-full items-center gap-2 rounded-sm px-2.5 py-1.5 text-[12px] text-text outline-none cursor-default select-none",
  "data-[highlighted]:bg-hover data-[disabled]:opacity-40 data-[disabled]:cursor-not-allowed",
);

/** Renders a top-level trigger label with the first letter underlined as a
 * typeahead hint — once the menubar has focus, pressing that letter jumps
 * to the corresponding menu. The underline is muted so it reads as a hint,
 * not as emphasis. */
function TriggerLabel({ label }: { label: string }) {
  return (
    <>
      <span className="underline decoration-text-muted/40 underline-offset-2">
        {label[0]}
      </span>
      {label.slice(1)}
    </>
  );
}

function MenuItem({
  onSelect,
  children,
  shortcut,
  icon: Icon,
  iconClassName,
  disabled,
  badge,
}: {
  onSelect: () => void;
  children: React.ReactNode;
  shortcut?: string;
  icon?: React.ComponentType<{ size?: number; className?: string }>;
  iconClassName?: string;
  disabled?: boolean;
  badge?: number;
}) {
  return (
    <Menubar.Item
      disabled={disabled}
      onSelect={onSelect}
      className={ITEM_CLASS}
    >
      {Icon && <Icon size={12} className={iconClassName ?? "text-text-muted"} />}
      <span className="flex-1 text-left">{children}</span>
      {badge !== undefined && badge > 0 && (
        <span className="min-w-[16px] rounded-sm bg-brand px-1 text-center text-[9px] font-bold text-white">
          {badge}
        </span>
      )}
      {shortcut && (
        <span className="ml-2 text-[10px] font-mono text-text-muted/40">
          {shortcut}
        </span>
      )}
    </Menubar.Item>
  );
}

function MenuSeparator() {
  return <Menubar.Separator className="my-1 mx-1.5 h-px bg-border-soft" />;
}

/** A submenu that opens on hover/right-arrow. Menubar.Sub handles all the
 * coordination (open-on-hover, close on sibling hover, arrow nav). */
function Submenu({
  label,
  icon: Icon,
  iconClassName,
  hint,
  contentClassName,
  children,
}: {
  label: React.ReactNode;
  icon?: React.ComponentType<{ size?: number; className?: string }>;
  iconClassName?: string;
  hint?: React.ReactNode;
  contentClassName?: string;
  children: React.ReactNode;
}) {
  return (
    <Menubar.Sub>
      <Menubar.SubTrigger className={ITEM_CLASS}>
        {Icon && <Icon size={12} className={iconClassName ?? "text-text-muted"} />}
        <span className="flex-1 text-left">{label}</span>
        {hint && (
          <span className="ml-2 text-[10px] text-text-muted/60">{hint}</span>
        )}
        <CaretRight size={10} className="text-text-muted/60" />
      </Menubar.SubTrigger>
      <Menubar.Portal>
        <Menubar.SubContent
          sideOffset={0}
          alignOffset={-5}
          className={cn(
            "z-50 min-w-[180px] rounded-lg border border-border/80 bg-surface p-1 shadow-md",
            contentClassName,
          )}
        >
          {children}
        </Menubar.SubContent>
      </Menubar.Portal>
    </Menubar.Sub>
  );
}

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

/** Render a registry command as a Menubar item. Looks the command up by id,
 * applies the category icon color, wires onSelect to run the action (Radix
 * auto-closes the menu on select). Keeps Header in sync with mobile and the
 * command palette — all three surfaces render the same action list. */
function CommandMenuItem({
  id,
  commands,
  label,
  badge,
}: {
  id: string;
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
      onSelect={() => {
        if (!enabled) return;
        cmd.action();
      }}
    >
      {displayLabel}
    </MenuItem>
  );
}

/** Toggles the bottom Manufacturability drawer. The chip in the status
 *  bar is the primary entry; this menu item is a discoverability hook. */
function ManufacturabilityMenuItem() {
  const drawerOpen = useDfmStore((s) => s.drawerOpen);
  const toggleDrawer = useDfmStore((s) => s.toggleDrawer);
  const Icon = COMMAND_ICONS["Wrench"];
  return (
    <MenuItem
      icon={Icon}
      iconClassName="text-amber-400"
      onSelect={() => toggleDrawer()}
    >
      {drawerOpen ? "Hide manufacturability" : "Manufacturability…"}
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

  const qualityLabel = (q: "draft" | "standard" | "high") =>
    t(
      q === "draft"
        ? "menu.view.ray_tracing.draft"
        : q === "standard"
          ? "menu.view.ray_tracing.standard"
          : "menu.view.ray_tracing.high",
    );

  return (
    <Submenu
      label={t("menu.view.ray_tracing")}
      icon={Sparkle}
      iconClassName={renderMode === "raytrace" ? "text-brand" : "text-text-muted"}
      hint={
        renderMode === "raytrace"
          ? qualityLabel(raytraceQuality)
          : t("menu.view.ray_tracing.off")
      }
    >
      <MenuItem
        onSelect={() => { if (renderMode === "raytrace") toggleRenderMode(); }}
      >
        <span className={renderMode === "standard" ? "text-brand" : "text-text"}>
          {t("menu.view.ray_tracing.off")}
        </span>
      </MenuItem>
      {(["draft", "standard", "high"] as const).map((q) => (
        <MenuItem
          key={q}
          onSelect={() => {
            if (renderMode !== "raytrace") toggleRenderMode();
            setRaytraceQuality(q);
          }}
        >
          <span
            className={
              renderMode === "raytrace" && raytraceQuality === q ? "text-brand" : "text-text"
            }
          >
            {qualityLabel(q)}
          </span>
        </MenuItem>
      ))}
      <MenuSeparator />
      <MenuItem onSelect={() => setRaytraceEdgesEnabled(!raytraceEdgesEnabled)}>
        <span className={raytraceEdgesEnabled ? "text-brand" : "text-text"}>
          {raytraceEdgesEnabled
            ? t("menu.view.ray_tracing.edges_on")
            : t("menu.view.ray_tracing.edges_off")}
        </span>
      </MenuItem>
    </Submenu>
  );
}

/** XR entries for the View menu. Always shown — disabled when the browser
 * doesn't report support, so users can see the capability exists. */
function XRMenuItems() {
  const vr = useXRSupportStore((s) => s.vr);
  const ar = useXRSupportStore((s) => s.ar);
  const presenting = useXRPresenting();

  if (presenting) {
    return (
      <MenuItem
        icon={Goggles}
        onSelect={() => xrStore.getState().session?.end()}
      >
        Exit XR
      </MenuItem>
    );
  }

  return (
    <>
      <MenuItem icon={Goggles} disabled={!vr} onSelect={() => xrStore.enterVR()}>
        Enter VR
      </MenuItem>
      <MenuItem icon={Goggles} disabled={!ar} onSelect={() => xrStore.enterAR()}>
        Enter AR
      </MenuItem>
    </>
  );
}

export function Header({ onAboutOpen, onProductOpen, onSave, onOpen, onShareOpen, onVersionHistoryOpen, children }: HeaderProps) {
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  // "Has a permanent identity" — false for anon Supabase sessions.
  const isSignedIn = !!user && !isAnonymous;
  const billingTier = useBillingStore((s) => s.snapshot?.tier ?? null);
  const [upgradeOpen, setUpgradeOpen] = useState(false);
  const [inputPrefsOpen, setInputPrefsOpen] = useState(false);
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  const [menuValue, setMenuValue] = useState("");
  const unreadChangelog = useChangelogStore((s) => s.getUnreadCount());
  const openChangelogPanel = useChangelogStore((s) => s.openPanel);
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

  // F10 activates the menu bar — opens File and hands focus to Menubar so
  // the user can arrow between menus or typeahead by letter (F/E/V/T/H).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (
        e.key === "F10" &&
        !e.shiftKey &&
        !e.ctrlKey &&
        !e.altKey &&
        !e.metaKey
      ) {
        e.preventDefault();
        setMenuValue((v) => (v ? "" : "file"));
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const commands = useAppCommands({
    onDismiss: () => {
      // Menubar auto-closes on item select — no explicit dismiss needed.
    },
    onAboutOpen,
    onSave,
    onOpen,
    surface: "desktop-menu",
  });

  // Under Tauri, the OS menu bar drives the same command registry — listen
  // for menu events and skip rendering the in-window Menubar triggers.
  const { tauri: nativeMenu, platform } = useCapabilities();
  useNativeMenu(commands);
  // Overlay titlebar on macOS puts the traffic lights on top of our row.
  // Reserve ~78px so the logo clears them; make the row a drag region so
  // the user can still drag the window by its toolbar.
  const macOverlay = nativeMenu && platform === "mac";

  const handleCommandPalette = () => {
    useUiStore.getState().setCommandPaletteOpen(true);
  };

  const handleExport = async (format: "stl" | "glb" | "step") => {
    const scene = useEngineStore.getState().scene;
    if (!scene) {
      useNotificationStore.getState().addToast("Nothing to export", "info");
      return;
    }
    try {
      const blob = await runJob(
        { verb: `Exporting ${format.toUpperCase()}` },
        () =>
          format === "stl" ? exportStlBlob(scene)
          : format === "glb" ? exportGltfBlob(scene)
          : exportStepBlob(scene),
      );
      downloadBlob(blob, `model.${format}`);
    } catch (err) {
      useNotificationStore
        .getState()
        .addToast(`Export failed: ${(err as Error).message}`, "error");
    }
  };

  // Examples live in /src/data and their file loader still goes through a
  // dispatched event — not in the command registry because each entry is
  // unique, not a shared command. Detail carries either an inline
  // `ExampleFile` (legacy / loon-built examples) or a `urdf` source
  // bundle (e.g. Unitree G1 / Go2) that the loader pipes through the
  // engine's URDF importer.
  const handleLoadExampleEntry = (ex: (typeof examples)[number]) => {
    if (ex.urdf) {
      window.dispatchEvent(
        new CustomEvent("vcad:load-example", { detail: { urdf: ex.urdf } }),
      );
    } else if (ex.file) {
      window.dispatchEvent(
        new CustomEvent("vcad:load-example", { detail: { file: ex.file } }),
      );
    }
  };

  return (
    <div
      data-tauri-drag-region={macOverlay ? "" : undefined}
      className={cn(
        "flex flex-col bg-surface select-none",
        macOverlay && "relative z-[60] pointer-events-auto",
      )}
    >
      {/* ─────────────────────────────────────────────────────── */}
      {/* Row 1: menu bar (logo + File/Edit/View/Tools/Help)     */}
      {/* ─────────────────────────────────────────────────────── */}
      <div
        data-tauri-drag-region={macOverlay ? "" : undefined}
        className={cn(
          "relative flex items-center gap-0 px-1 border-b border-border/30",
          macOverlay ? "h-9 pl-[78px]" : "h-7",
        )}
      >
        <div
          className="flex items-center pr-2 pl-1"
          data-tauri-drag-region={macOverlay ? "" : undefined}
        >
          <Tooltip content="About vcad" side="bottom">
            <button
              type="button"
              onClick={onAboutOpen}
              aria-label="About vcad"
              className="font-mono text-[13px] font-semibold text-text hover:text-brand transition-colors cursor-default outline-none"
            >
              vcad<span className="text-brand">.</span>
            </button>
          </Tooltip>
        </div>

        {/* Centered document-identity strip — filename, save state, scope breadcrumb */}
        <div className="absolute left-1/2 -translate-x-1/2">
          <DocTitle macOverlay={macOverlay} />
        </div>

        <Menubar.Root
          value={menuValue}
          onValueChange={setMenuValue}
          loop
          className={cn(
            "flex items-center gap-0 min-w-0 overflow-x-auto no-scrollbar",
            nativeMenu && "hidden",
          )}
        >
          <Menubar.Menu value="file">
            <Menubar.Trigger className={TRIGGER_CLASS}>
              <TriggerLabel label={t("menu.file")} />
            </Menubar.Trigger>
            <Menubar.Portal>
              <Menubar.Content
                align="start"
                sideOffset={2}
                alignOffset={-3}
                className={CONTENT_CLASS}
                onCloseAutoFocus={(e) => e.preventDefault()}
              >
                <CommandMenuItem id="new-document" commands={commands} />
                <CommandMenuItem id="open" commands={commands} />
                <CommandMenuItem id="open-cloud" commands={commands} />
                <MenuSeparator />
                <CommandMenuItem id="save" commands={commands} />
                <MenuItem
                  onSelect={onShareOpen}
                  icon={LinkIcon}
                  iconClassName="text-sky-400"
                  disabled={!isSignedIn}
                >
                  {t("menu.file.share_link")}
                </MenuItem>
                <MenuItem
                  onSelect={onVersionHistoryOpen}
                  disabled={!isSignedIn}
                >
                  {t("menu.file.version_history")}
                </MenuItem>
                <MenuSeparator />
                <Submenu label={t("menu.file.export")} icon={Export} iconClassName="text-sky-400">
                  <MenuItem onSelect={() => handleExport("stl")}>STL</MenuItem>
                  <MenuItem onSelect={() => handleExport("glb")}>GLB</MenuItem>
                  <MenuItem onSelect={() => handleExport("step")}>STEP</MenuItem>
                </Submenu>
                <MenuSeparator />
                <Submenu
                  label={t("menu.file.examples")}
                  icon={BookOpen}
                  iconClassName="text-sky-400"
                  contentClassName="min-w-[200px] max-h-[60vh] overflow-y-auto"
                >
                  {examples.map((ex) => (
                    <MenuItem
                      key={ex.id}
                      onSelect={() => handleLoadExampleEntry(ex)}
                    >
                      {ex.name}
                    </MenuItem>
                  ))}
                </Submenu>
              </Menubar.Content>
            </Menubar.Portal>
          </Menubar.Menu>

          <Menubar.Menu value="edit">
            <Menubar.Trigger className={TRIGGER_CLASS}>
              <TriggerLabel label={t("menu.edit")} />
            </Menubar.Trigger>
            <Menubar.Portal>
              <Menubar.Content
                align="start"
                sideOffset={2}
                alignOffset={-3}
                className={CONTENT_CLASS}
                onCloseAutoFocus={(e) => e.preventDefault()}
              >
                <CommandMenuItem id="undo" commands={commands} />
                <CommandMenuItem id="redo" commands={commands} />
                <MenuSeparator />
                <CommandMenuItem id="copy" commands={commands} />
                <CommandMenuItem id="paste" commands={commands} />
                <CommandMenuItem id="duplicate" commands={commands} />
                <CommandMenuItem id="delete" commands={commands} />
                <MenuSeparator />
                <CommandMenuItem id="select-all" commands={commands} />
                <CommandMenuItem id="deselect" commands={commands} />
              </Menubar.Content>
            </Menubar.Portal>
          </Menubar.Menu>

          <Menubar.Menu value="view">
            <Menubar.Trigger className={TRIGGER_CLASS}>
              <TriggerLabel label={t("menu.view")} />
            </Menubar.Trigger>
            <Menubar.Portal>
              <Menubar.Content
                align="start"
                sideOffset={2}
                alignOffset={-3}
                className={CONTENT_CLASS}
                onCloseAutoFocus={(e) => e.preventDefault()}
              >
                <CommandMenuItem id="toggle-sidebar" commands={commands} />
                <CommandMenuItem id="toggle-chat" commands={commands} />
                <CommandMenuItem id="toggle-status-bar" commands={commands} />
                <CommandMenuItem id="toggle-devtools" commands={commands} />
                <MenuSeparator />
                <CommandMenuItem id="camera-isometric" commands={commands} />
                <CommandMenuItem id="camera-top" commands={commands} />
                <CommandMenuItem id="camera-front" commands={commands} />
                <CommandMenuItem id="camera-right" commands={commands} />
                <CommandMenuItem id="camera-fit" commands={commands} />
                <MenuSeparator />
                <CommandMenuItem id="toggle-wireframe" commands={commands} />
                <CommandMenuItem id="toggle-grid-snap" commands={commands} />
                <MenuSeparator />
                <RayTracingSubmenu />
                <MenuItem
                  icon={Mouse}
                  onSelect={() => setInputPrefsOpen(true)}
                >
                  {t("menu.view.input_preferences")}
                </MenuItem>
                <XRMenuItems />
                <MenuSeparator />
                <CommandMenuItem id="cycle-theme" commands={commands} />
              </Menubar.Content>
            </Menubar.Portal>
          </Menubar.Menu>

          <Menubar.Menu value="tools">
            <Menubar.Trigger className={TRIGGER_CLASS}>
              <TriggerLabel label={t("menu.tools")} />
            </Menubar.Trigger>
            <Menubar.Portal>
              <Menubar.Content
                align="start"
                sideOffset={2}
                alignOffset={-3}
                className={CONTENT_CLASS}
                onCloseAutoFocus={(e) => e.preventDefault()}
              >
                <CommandMenuItem id="command-palette" commands={commands} />
                <MenuSeparator />
                <CommandMenuItem id="new-sketch" commands={commands} />
                <MenuSeparator />
                <ManufacturabilityMenuItem />
                <MenuSeparator />
                <CommandMenuItem id="open-slicer" commands={commands} />
                <CommandMenuItem id="open-cam" commands={commands} />
              </Menubar.Content>
            </Menubar.Portal>
          </Menubar.Menu>

          <Menubar.Menu value="help">
            <Menubar.Trigger className={TRIGGER_CLASS}>
              <TriggerLabel label={t("menu.help")} />
            </Menubar.Trigger>
            <Menubar.Portal>
              <Menubar.Content
                align="start"
                sideOffset={2}
                alignOffset={-3}
                className={CONTENT_CLASS}
                onCloseAutoFocus={(e) => e.preventDefault()}
              >
                <CommandMenuItem id="about" commands={commands} />
                <Menubar.Item
                  className={ITEM_CLASS}
                  onSelect={() => onProductOpen()}
                >
                  {t("menu.help.vcad_pro")}
                </Menubar.Item>
                <CommandMenuItem
                  id="whats-new"
                  commands={commands}
                  badge={unreadChangelog}
                />
                <MenuSeparator />
                <CommandMenuItem id="open-docs" commands={commands} />
                <CommandMenuItem id="open-github" commands={commands} />
                <CommandMenuItem id="open-discord" commands={commands} />
              </Menubar.Content>
            </Menubar.Portal>
          </Menubar.Menu>
        </Menubar.Root>

        <div
          className="flex-1"
          data-tauri-drag-region={macOverlay ? "" : undefined}
        />

        {/* Right cluster: search · changelog bell · auth. Search drops below
            sm; bell drops below md. Auth/user always present. */}
        <Tooltip content="Search or ask AI… (⌘K)" side="bottom">
          <button
            type="button"
            onClick={handleCommandPalette}
            aria-label="Search or ask AI"
            className={cn(
              "relative shrink-0 hidden sm:flex items-center justify-center w-7 h-6",
              "text-text-muted hover:text-text hover:bg-hover",
            )}
          >
            <MagnifyingGlass size={13} />
          </button>
        </Tooltip>
        <Tooltip
          content={
            unreadChangelog > 0
              ? `What's new — ${unreadChangelog} unread`
              : "What's new"
          }
          side="bottom"
        >
          <button
            type="button"
            onClick={openChangelogPanel}
            aria-label={
              unreadChangelog > 0
                ? `What's new — ${unreadChangelog} unread`
                : "What's new"
            }
            className={cn(
              "relative shrink-0 hidden md:flex items-center justify-center w-7 h-6",
              "text-text-muted hover:text-text hover:bg-hover",
            )}
          >
            <Bell size={13} weight={unreadChangelog > 0 ? "fill" : "regular"} />
            {unreadChangelog > 0 && (
              <span
                className="absolute top-1 right-1.5 w-1.5 h-1.5 rounded-full bg-brand"
                aria-hidden="true"
              />
            )}
          </button>
        </Tooltip>
        <Tooltip content="Send feedback" side="bottom">
          <button
            type="button"
            onClick={() => setFeedbackOpen(true)}
            aria-label="Send feedback"
            className={cn(
              "relative flex items-center justify-center w-7 h-6",
              "text-text-muted hover:text-text hover:bg-hover",
            )}
          >
            <Megaphone size={13} />
          </button>
        </Tooltip>
        <SignInButton
          variant="icon-text"
          className={cn(
            "shrink-0 flex items-center justify-center gap-1.5 h-6 px-2 text-[11px] font-medium",
            "text-text-muted hover:text-text hover:bg-hover",
          )}
        />
        <UserMenu
          onSyncNow={() => triggerSync()}
          planLabel={billingTier ? TIERS[billingTier].name : undefined}
          onUpgrade={
            billingTier !== "max" ? () => setUpgradeOpen(true) : undefined
          }
          onManageSubscription={
            billingTier && billingTier !== "free"
              ? () => {
                  void openCustomerPortal().catch((err) => {
                    console.error("[header] portal error:", err);
                  });
                }
              : undefined
          }
        />
      </div>
      <UpgradeModal
        open={upgradeOpen}
        onOpenChange={setUpgradeOpen}
        reason="manual"
      />
      <Suspense fallback={null}>
        <InputPreferencesDialog
          open={inputPrefsOpen}
          onOpenChange={setInputPrefsOpen}
        />
      </Suspense>
      <Suspense fallback={null}>
        {feedbackOpen && (
          <FeedbackModal open={feedbackOpen} onOpenChange={setFeedbackOpen} />
        )}
      </Suspense>

      {/* ─────────────────────────────────────────────────────── */}
      {/* Row 2+: tool palette (tab strip + icon row) docked under */}
      {/* ─────────────────────────────────────────────────────── */}
      {children}
    </div>
  );
}
