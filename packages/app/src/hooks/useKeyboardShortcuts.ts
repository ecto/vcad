import { useEffect } from "react";
import { useUiStore, useDocumentStore, useSketchStore, useChatStore } from "@vcad/core";
import { useElectronicsStore } from "../stores/electronics-store";
import { useNotificationStore } from "../stores/notification-store";
import { useLogStore } from "../stores/log-store";
import { useChangelogStore } from "../stores/changelog-store";

// Track last Escape time for double-tap emergency exit
let lastEscapeTime = 0;

export function useKeyboardShortcuts() {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // The new useKeybindingDispatcher runs in the capture phase and
      // preventDefaults any event it successfully dispatches through the
      // Rust registry. Bail here so we don't double-fire for migrated
      // bindings. Commands still owned by this hook (sketch tool picks,
      // chat shortcuts, etc.) keep their existing behavior.
      if (e.defaultPrevented) return;

      // Ignore when typing in inputs
      const target = e.target as HTMLElement;
      if (
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable
      ) {
        return;
      }

      // Electronics mode has its own keyboard handler — only allow
      // modifier-based shortcuts (Cmd+S, Cmd+Z, etc.) to pass through.
      if (useElectronicsStore.getState().active && !e.ctrlKey && !e.metaKey) {
        return;
      }

      // Read-only share session: intercept known mutation-class keys so the
      // viewer gets the fork prompt instead of silently switching transform
      // mode or triggering deletes. View-only keys (escape, navigation, view
      // toggles, camera) fall through and work normally.
      const readOnlyShare = useUiStore.getState().readOnlyShare;
      if (readOnlyShare) {
        const isMod = e.ctrlKey || e.metaKey;
        const key = e.key.toLowerCase();
        const isMutationKey =
          // Delete / backspace — would remove features
          e.key === "Delete" ||
          e.key === "Backspace" ||
          // Transform mode switches — harmless but confusing in read-only
          (!isMod && (key === "m" || key === "r" || key === "s")) ||
          // Cmd/Ctrl+D duplicate
          (isMod && key === "d") ||
          // Sketch tools / shape tools (unmodified)
          (!isMod && (key === "l" || key === "c"));
        if (isMutationKey) {
          e.preventDefault();
          window.dispatchEvent(
            new CustomEvent("vcad:fork-prompt", { detail: readOnlyShare }),
          );
          return;
        }
      }

      const {
        selectedPartIds,
        clearSelection,
        setTransformMode,
        toggleWireframe,
        toggleGridSnap,
        toggleFeatureTree,
      } = useUiStore.getState();
      const { undo, redo } = useDocumentStore.getState();

      const mod = e.ctrlKey || e.metaKey;

      // ── Borland-style function key bindings (alt paths kept here) ───
      // F1/F6/Cmd+K/Cmd+S/Cmd+O are now claimed by the Rust registry via
      // useKeybindingDispatcher. F2/F3/F5/F10 stay as alternative
      // function-key bindings until they're added to the registry too.
      if (e.key === "F2") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:save"));
        return;
      }
      if (e.key === "F3") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:open"));
        return;
      }
      if (e.key === "F5") {
        e.preventDefault();
        toggleWireframe();
        return;
      }
      if (e.key === "F10") {
        e.preventDefault();
        useUiStore.getState().setCommandPaletteOpen(true);
        return;
      }


      // Toggle feature tree: Cmd+1
      if (mod && e.key === "1") {
        e.preventDefault();
        toggleFeatureTree();
        return;
      }

      // Log viewer: ~ (backtick)
      if (e.key === "`") {
        e.preventDefault();
        useLogStore.getState().togglePanel();
        return;
      }

      // What's New panel: ?
      if (e.key === "?" && !mod) {
        e.preventDefault();
        useChangelogStore.getState().togglePanel();
        return;
      }

      // AI / Chat: Cmd+J (same as Cmd+K)
      if (mod && e.key === "j") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:open-chat"));
        return;
      }

      // Toggle chat sidebar: Cmd+Shift+L
      if (e.key === "l" && (e.metaKey || e.ctrlKey) && e.shiftKey) {
        e.preventDefault();
        useChatStore.getState().toggleOpen();
        return;
      }

      // Undo: Ctrl/Cmd+Z
      if (mod && !e.shiftKey && e.key === "z") {
        e.preventDefault();
        // While in an active sketch, undo mutates sketch-local history
        // (drawn segments, constraints) rather than the document history.
        if (useSketchStore.getState().active) {
          useSketchStore.getState().undoSketch();
        } else {
          undo();
        }
        return;
      }

      // Redo: Ctrl/Cmd+Shift+Z
      if (mod && e.shiftKey && e.key === "z") {
        e.preventDefault();
        if (useSketchStore.getState().active) {
          useSketchStore.getState().redoSketch();
        } else {
          redo();
        }
        return;
      }

      // Document picker: Alt+O or Ctrl/Cmd+Shift+O — not in the registry
      // (it's a separate flow from the regular Open dispatch).
      if ((e.altKey && e.key === "o") || (mod && e.shiftKey && e.key === "o")) {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:documents"));
        return;
      }

      // Cmd+D / Cmd+C / Cmd+V / Cmd+S / Cmd+O / Cmd+Shift+U/D/I are now
      // dispatched by useKeybindingDispatcher via the Rust registry.

      // Command palette: S (no modifiers, not in sketch)
      if ((e.key === "s" || e.key === "S") && !mod && !e.shiftKey && !e.altKey) {
        const { active, faceSelectionMode } = useSketchStore.getState();
        if (!active && !faceSelectionMode) {
          e.preventDefault();
          useUiStore.getState().setCommandPaletteOpen(true);
          return;
        }
      }

      // Sketch tool shortcuts: R/C/L pick the drawing tool while a sketch
      // is active. These must come before the transform-mode bindings
      // below so "R" doesn't get captured as "rotate" in sketch mode.
      if (useSketchStore.getState().active && !mod && !e.shiftKey && !e.altKey) {
        const key = e.key.toLowerCase();
        if (key === "r" || key === "c" || key === "l") {
          e.preventDefault();
          const tool = key === "r" ? "rectangle" : key === "c" ? "circle" : "line";
          useSketchStore.getState().setTool(tool);
          return;
        }
      }

      // Transform modes (only outside sketch mode — those keys mean
      // "pick a drawing tool" while sketching).
      if ((e.key === "m" || e.key === "M") && !useSketchStore.getState().active) {
        setTransformMode("translate");
        return;
      }
      if ((e.key === "r" || e.key === "R") && !useSketchStore.getState().active) {
        setTransformMode("rotate");
        return;
      }
      if (
        e.shiftKey &&
        (e.key === "s" || e.key === "S") &&
        !mod &&
        !useSketchStore.getState().active
      ) {
        setTransformMode("scale");
        return;
      }

      // X (wireframe toggle) is now handled by the registry dispatcher.

      // Toggle ray tracing: Alt+R
      if (e.altKey && (e.key === "r" || e.key === "R")) {
        e.preventDefault();
        const { raytraceAvailable, toggleRenderMode } = useUiStore.getState();
        if (raytraceAvailable) {
          toggleRenderMode();
        }
        return;
      }

      // Toggle grid snap
      if (e.key === "g" || e.key === "G") {
        toggleGridSnap();
        return;
      }

      // Quick extrude: E (when in sketch mode with segments)
      if ((e.key === "e" || e.key === "E") && !mod) {
        const { active, segments } = useSketchStore.getState();
        if (active && segments.length > 0) {
          e.preventDefault();
          window.dispatchEvent(new CustomEvent("vcad:sketch-extrude"));
          return;
        }
      }

      // Focus camera on selection
      if (e.key === "f" || e.key === "F") {
        if (selectedPartIds.size > 0) {
          window.dispatchEvent(new CustomEvent("vcad:focus-selection"));
        }
        return;
      }

      // Delete (Delete/Backspace) is now handled by the registry dispatcher
      // via the `delete` command (when=has_selection && !input_focused).

      // Escape: cancel in-progress tool, exit sketch mode, cancel face selection, or deselect
      if (e.key === "Escape") {
        const now = Date.now();
        const isDoubleTap = now - lastEscapeTime < 400; // 400ms window
        lastEscapeTime = now;

        const {
          active,
          faceSelectionMode,
          pendingExit,
          points,
          requestExit,
          cancelExit,
          cancelFaceSelection,
          exitSketchMode,
          validateState,
          setTool,
        } = useSketchStore.getState();

        // Run state validation to fix any inconsistent states
        validateState();

        // Double-tap: force exit from any sketch state
        if (isDoubleTap) {
          if (active || faceSelectionMode || pendingExit) {
            exitSketchMode();
            cancelFaceSelection();
            useNotificationStore.getState().addToast("Sketch cancelled", "info");
            return;
          }
        }

        // Cancel face selection mode
        if (faceSelectionMode) {
          cancelFaceSelection();
          useNotificationStore.getState().addToast("Face selection cancelled", "info");
          return;
        }

        if (active) {
          // If mid-draw (have in-progress points), cancel the current tool operation
          if (points.length > 0) {
            // setTool resets points to []
            setTool(useSketchStore.getState().tool);
            return;
          }

          // If confirmation dialog is showing, cancel it
          if (pendingExit) {
            cancelExit();
            return;
          }
          // Request exit - returns true if exited immediately (empty sketch)
          const exited = requestExit();
          if (exited) {
            useNotificationStore.getState().addToast("Sketch cancelled", "info");
          }
          // If not exited, confirmation dialog will show in SketchConfirmationCorner
        } else {
          clearSelection();
        }
        return;
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);
}
