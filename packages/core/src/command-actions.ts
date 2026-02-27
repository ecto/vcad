import type { CommandActions } from "./commands.js";
import { useDocumentStore } from "./stores/document-store.js";
import { useUiStore } from "./stores/ui-store.js";

/**
 * Create default command actions that wire into the shared stores.
 *
 * Covers primitives, booleans, transforms, edit, and view toggles.
 * File and assembly/modify actions are left as no-ops (override them per-platform).
 *
 * @param onDismiss Called after each command to dismiss the palette/input.
 */
export function createDefaultCommandActions(
  onDismiss: () => void,
): CommandActions {
  const docState = () => useDocumentStore.getState();
  const uiState = () => useUiStore.getState();

  return {
    addPrimitive: (kind) => {
      const partId = docState().addPrimitive(kind);
      uiState().select(partId);
      onDismiss();
    },
    applyBoolean: (type) => {
      const ids = Array.from(uiState().selectedPartIds);
      if (ids.length === 2) {
        const newId = docState().applyBoolean(type, ids[0]!, ids[1]!);
        if (newId) uiState().select(newId);
      }
      onDismiss();
    },
    setTransformMode: (mode) => {
      uiState().setTransformMode(mode);
      onDismiss();
    },
    undo: () => {
      docState().undo();
      onDismiss();
    },
    redo: () => {
      docState().redo();
      onDismiss();
    },
    toggleWireframe: () => {
      uiState().toggleWireframe();
      onDismiss();
    },
    toggleGridSnap: () => {
      uiState().toggleGridSnap();
      onDismiss();
    },
    toggleFeatureTree: () => {
      uiState().toggleFeatureTree();
      onDismiss();
    },
    save: () => onDismiss(),
    open: () => onDismiss(),
    exportStl: () => onDismiss(),
    exportGlb: () => onDismiss(),
    openAbout: () => onDismiss(),
    deleteSelected: () => {
      const ui = uiState();
      for (const id of ui.selectedPartIds) {
        docState().removePart(id);
      }
      ui.clearSelection();
      onDismiss();
    },
    duplicateSelected: () => {
      const ui = uiState();
      if (ui.selectedPartIds.size > 0) {
        const ids = Array.from(ui.selectedPartIds);
        const newIds = docState().duplicateParts(ids);
        ui.selectMultiple(newIds);
      }
      onDismiss();
    },
    deselectAll: () => {
      uiState().clearSelection();
      onDismiss();
    },
    hasTwoSelected: () => uiState().selectedPartIds.size === 2,
    hasSelection: () => uiState().selectedPartIds.size > 0,
    hasParts: () => docState().parts.length > 0,
    canUndo: () => docState()._crdtEngine?.can_undo() ?? false,
    canRedo: () => docState()._crdtEngine?.can_redo() ?? false,
  };
}
