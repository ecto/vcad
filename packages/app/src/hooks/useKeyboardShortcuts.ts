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

      const {
        selectedPartIds,
        clearSelection,
        setTransformMode,
        toggleWireframe,
        toggleGridSnap,
        copyToClipboard,
        toggleFeatureTree,
      } = useUiStore.getState();
      const { undo, redo, removePart, duplicateParts, applyBoolean } =
        useDocumentStore.getState();

      const mod = e.ctrlKey || e.metaKey;

      // Chat: Cmd+K
      if (mod && e.key === "k") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:open-chat"));
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
        undo();
        return;
      }

      // Redo: Ctrl/Cmd+Shift+Z
      if (mod && e.shiftKey && e.key === "z") {
        e.preventDefault();
        redo();
        return;
      }

      // Duplicate: Ctrl/Cmd+D
      if (mod && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        if (selectedPartIds.size > 0) {
          const ids = Array.from(selectedPartIds);
          const newIds = duplicateParts(ids);
          useUiStore.getState().selectMultiple(newIds);
        }
        return;
      }

      // Copy: Ctrl/Cmd+C
      if (mod && !e.shiftKey && e.key === "c") {
        e.preventDefault();
        if (selectedPartIds.size === 0) {
          useNotificationStore.getState().addToast("Nothing to copy", "info");
          return;
        }
        copyToClipboard(Array.from(selectedPartIds));
        const count = selectedPartIds.size;
        useNotificationStore
          .getState()
          .addToast(`Copied ${count} part${count > 1 ? "s" : ""}`, "success");
        return;
      }

      // Paste: Ctrl/Cmd+V
      if (mod && !e.shiftKey && e.key === "v") {
        const { clipboard } = useUiStore.getState();
        if (clipboard.length > 0) {
          e.preventDefault();
          const newIds = duplicateParts(clipboard);
          useUiStore.getState().selectMultiple(newIds);
          const count = newIds.length;
          useNotificationStore
            .getState()
            .addToast(`Pasted ${count} part${count > 1 ? "s" : ""}`, "success");
        }
        return;
      }

      // Save: Ctrl/Cmd+S
      if (mod && !e.shiftKey && e.key === "s") {
        e.preventDefault();
        // Dispatched as custom event, handled by App.tsx
        window.dispatchEvent(new CustomEvent("vcad:save"));
        return;
      }

      // Open file: Ctrl/Cmd+O
      if (mod && !e.shiftKey && e.key === "o") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:open"));
        return;
      }

      // Document picker: Alt+O or Ctrl/Cmd+Shift+O
      if ((e.altKey && e.key === "o") || (mod && e.shiftKey && e.key === "o")) {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("vcad:documents"));
        return;
      }

      // Boolean shortcuts (2 selected)
      if (mod && e.shiftKey && selectedPartIds.size === 2) {
        const ids = Array.from(selectedPartIds);
        const keyLower = e.key.toLowerCase();
        if (keyLower === "u") {
          e.preventDefault();
          const newId = applyBoolean("union", ids[0]!, ids[1]!);
          if (newId) useUiStore.getState().select(newId);
          return;
        }
        if (keyLower === "d") {
          e.preventDefault();
          const newId = applyBoolean("difference", ids[0]!, ids[1]!);
          if (newId) useUiStore.getState().select(newId);
          return;
        }
        if (keyLower === "i") {
          e.preventDefault();
          const newId = applyBoolean("intersection", ids[0]!, ids[1]!);
          if (newId) useUiStore.getState().select(newId);
          return;
        }
      }

      // Command palette: S (no modifiers, not in sketch)
      if ((e.key === "s" || e.key === "S") && !mod && !e.shiftKey && !e.altKey) {
        const { active, faceSelectionMode } = useSketchStore.getState();
        if (!active && !faceSelectionMode) {
          e.preventDefault();
          useUiStore.getState().setCommandPaletteOpen(true);
          return;
        }
      }

      // Transform modes
      if (e.key === "m" || e.key === "M") {
        setTransformMode("translate");
        return;
      }
      if (e.key === "r" || e.key === "R") {
        setTransformMode("rotate");
        return;
      }
      if (e.shiftKey && (e.key === "s" || e.key === "S") && !mod) {
        setTransformMode("scale");
        return;
      }

      // Toggle wireframe
      if (e.key === "x" || e.key === "X") {
        toggleWireframe();
        return;
      }

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

      // Delete selected
      if (
        (e.key === "Delete" || e.key === "Backspace") &&
        selectedPartIds.size > 0
      ) {
        e.preventDefault();
        const ids = Array.from(selectedPartIds);
        for (const id of ids) {
          removePart(id);
        }
        clearSelection();
        return;
      }

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
          // If not exited, confirmation dialog will show in SketchToolbar
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
