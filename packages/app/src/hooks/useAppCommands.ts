import { useMemo } from "react";
import {
  createCommandRegistry,
  createDefaultCommandActions,
  useDocumentStore,
  useUiStore,
  useEngineStore,
  useChatStore,
  useSketchStore,
  exportStlBlob,
  exportGltfBlob,
  exportStepBlob,
  type CommandRegistry,
} from "@vcad/core";
import { useChangelogStore } from "@/stores/changelog-store";
import { useLogStore } from "@/stores/log-store";
import { useSlicerStore } from "@/stores/slicer-store";
import { useCamStore } from "@/stores/cam-store";
import { useNotificationStore } from "@/stores/notification-store";
import { downloadBlob } from "@/lib/download";

interface UseAppCommandsProps {
  /** Called after any command fires — use this to dismiss an open palette/sheet. */
  onDismiss: () => void;
  /** Opens the About dialog (modal state lives at the App level). */
  onAboutOpen: () => void;
  /** Save action — dispatches a window event by default if unspecified. */
  onSave?: () => void;
  /** Open action — dispatches a window event by default if unspecified. */
  onOpen?: () => void;
}

/**
 * Builds the full CommandRegistry for vcad's desktop app. Shared by
 * CommandPalette (⌘K search UI) and MobileShell (hamburger menu grouped by
 * category), so both surfaces draw from the same list and any new command
 * lands on both for free.
 *
 * Callers supply the small set of actions that can't be reached from stores
 * (About dialog, Save/Open dispatchers) — everything else is wired to the
 * shared zustand stores and their getState() handles.
 */
export function useAppCommands({
  onDismiss,
  onAboutOpen,
  onSave,
  onOpen,
}: UseAppCommandsProps): CommandRegistry {
  return useMemo(() => {
    const base = createDefaultCommandActions(onDismiss);

    const doExport = (format: "stl" | "glb" | "step") => {
      const scene = useEngineStore.getState().scene;
      if (!scene) {
        useNotificationStore.getState().addToast("Nothing to export", "info");
        return;
      }
      try {
        const blob =
          format === "stl"
            ? exportStlBlob(scene)
            : format === "glb"
              ? exportGltfBlob(scene)
              : exportStepBlob(scene);
        downloadBlob(blob, `model.${format}`);
      } catch (err) {
        useNotificationStore
          .getState()
          .addToast(`Export failed: ${(err as Error).message}`, "error");
      }
      onDismiss();
    };

    return createCommandRegistry({
      ...base,
      // App-specific overrides of core actions
      addPrimitive: (kind) => {
        const partId = useDocumentStore.getState().addPrimitive(kind);
        useUiStore.getState().select(partId);
        useUiStore.getState().setTransformMode("translate");
        onDismiss();
      },
      save: () => {
        if (onSave) onSave();
        else window.dispatchEvent(new CustomEvent("vcad:save"));
        onDismiss();
      },
      open: () => {
        if (onOpen) onOpen();
        else window.dispatchEvent(new CustomEvent("vcad:open"));
        onDismiss();
      },
      exportStl: () => doExport("stl"),
      exportGlb: () => doExport("glb"),
      openAbout: () => {
        onAboutOpen();
        onDismiss();
      },

      // File extras
      newDocument: () => {
        if (
          useDocumentStore.getState().isDirty &&
          !window.confirm("Discard unsaved changes and start a new document?")
        ) {
          return;
        }
        useDocumentStore.getState().newDocument(crypto.randomUUID(), "Untitled");
        onDismiss();
      },
      openFromCloud: () => {
        window.dispatchEvent(new CustomEvent("vcad:documents"));
        onDismiss();
      },
      exportStep: () => doExport("step"),

      // Edit extras
      copy: () => {
        const ui = useUiStore.getState();
        if (ui.selectedPartIds.size === 0) return;
        ui.copyToClipboard(Array.from(ui.selectedPartIds));
        useNotificationStore
          .getState()
          .addToast(
            `Copied ${ui.selectedPartIds.size} part${ui.selectedPartIds.size > 1 ? "s" : ""}`,
            "success",
          );
        onDismiss();
      },
      paste: () => {
        const ui = useUiStore.getState();
        if (ui.clipboard.length === 0) return;
        const newIds = useDocumentStore.getState().duplicateParts(ui.clipboard);
        ui.selectMultiple(newIds);
        onDismiss();
      },
      selectAll: () => {
        const parts = useDocumentStore.getState().parts;
        useUiStore.getState().selectMultiple(parts.map((p) => p.id));
        onDismiss();
      },

      // View extras
      cameraFit: () => {
        window.dispatchEvent(new CustomEvent("vcad:camera-fit"));
        onDismiss();
      },
      cameraPreset: (preset) => {
        window.dispatchEvent(new CustomEvent(`vcad:camera-${preset}`));
        onDismiss();
      },
      toggleChatSidebar: () => {
        useChatStore.getState().toggleOpen();
        onDismiss();
      },
      toggleStatusBar: () => {
        useUiStore.getState().toggleStatusBar();
        onDismiss();
      },
      toggleDevTools: () => {
        useLogStore.getState().togglePanel();
        onDismiss();
      },
      cycleTheme: () => {
        const ui = useUiStore.getState();
        const next =
          ui.theme === "dark" ? "light" : ui.theme === "light" ? "system" : "dark";
        ui.setTheme(next);
        onDismiss();
      },

      // Tools extras
      openCommandPalette: () => {
        useUiStore.getState().setCommandPaletteOpen(true);
        onDismiss();
      },
      newSketch: () => {
        useSketchStore.getState().enterFaceSelectionMode();
        useNotificationStore.getState().addToast("Select a face to sketch on", "info");
        onDismiss();
      },
      openSlicer: () => {
        useSlicerStore.getState().openPrintPanel();
        onDismiss();
      },
      openCam: () => {
        useCamStore.getState().openCamPanel();
        onDismiss();
      },

      // Help extras
      openWhatsNew: () => {
        useChangelogStore.getState().openPanel();
        onDismiss();
      },
      openDocs: () => {
        window.open("https://docs.vcad.io", "_blank");
        onDismiss();
      },
      openGithub: () => {
        window.open("https://github.com/ecto/vcad", "_blank");
        onDismiss();
      },
      openDiscord: () => {
        window.open("https://discord.gg/ZU8QHnFAc2", "_blank");
        onDismiss();
      },
    });
  }, [onDismiss, onAboutOpen, onSave, onOpen]);
}
