/**
 * Single global keydown dispatcher driven by the shared Rust keybinding
 * registry.
 *
 * This hook:
 * 1. Loads the wasm-backed `KeybindingRegistry` lazily.
 * 2. Listens for `keydown` events on `window`.
 * 3. Normalizes each event into a `Chord`, reads store state to build a
 *    `WhenContext`, and asks the registry to resolve it to a command id.
 * 4. If a command matches, invokes its action via the local action map and
 *    calls `preventDefault()`.
 *
 * Runs alongside the legacy `useKeyboardShortcuts` hook during the
 * migration. The legacy hook checks `e.defaultPrevented` and bails out, so
 * any chord the registry successfully dispatches no longer double-fires.
 * Bindings not yet migrated into the Rust registry continue to live in
 * `useKeyboardShortcuts` and will move over as this slice expands.
 */

import { useEffect, useMemo, useRef } from "react";
import {
  chordFromEvent,
  buildWhenContext,
  isInputFocused,
  getKeybindingRegistry,
  useUiStore,
  useDocumentStore,
  useSketchStore,
  useSimulationStore,
  type KeybindingRegistry,
  type KeybindingMode as AppMode,
} from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useAppCommands } from "@/hooks/useAppCommands";

interface UseKeybindingDispatcherProps {
  onAboutOpen: () => void;
  onSave: () => void;
  onOpen: () => void;
}

/** Alias map translating Rust-side command ids (snake_case, short) to the
 * TS command-registry ids (kebab-case, longer). The two sources grew up
 * independently — Rust in `vcad-app::commands` to serve the TUI, TS in
 * `@vcad/core/commands.ts` to serve the web. They'll converge, but until
 * then the dispatcher maps across at lookup time. */
const ID_ALIASES: Record<string, string> = {
  // Primitives
  cube: "add-box",
  cylinder: "add-cylinder",
  sphere: "add-sphere",
  // cone: no TS equivalent yet
  // Transform modes
  translate: "mode-move",
  rotate: "mode-rotate",
  scale: "mode-scale",
  // Booleans
  union: "boolean-union",
  difference: "boolean-difference",
  intersection: "boolean-intersection",
  // Modify
  fillet: "apply-fillet",
  chamfer: "apply-chamfer",
  shell: "apply-shell",
  linear_pattern: "apply-linear-pattern",
  circular_pattern: "apply-circular-pattern",
  mirror: "apply-mirror",
  // File
  new: "new-document",
  export_stl: "export-stl",
  export_glb: "export-glb",
  export_step: "export-step",
  // View
  toggle_sidebar: "toggle-sidebar",
  toggle_chat: "toggle-chat",
  toggle_devtools: "toggle-devtools",
  toggle_wireframe: "toggle-wireframe",
  toggle_grid_snap: "toggle-grid-snap",
  cycle_theme: "cycle-theme",
  camera_iso: "camera-isometric",
  camera_top: "camera-top",
  camera_front: "camera-front",
  camera_right: "camera-right",
  camera_fit: "camera-fit",
  // Tools
  palette: "command-palette",
  sketch: "new-sketch",
  // Help
  open_docs: "open-docs",
  open_github: "open-github",
  open_discord: "open-discord",
  // quit, save, undo, redo, delete, duplicate, copy, paste, select_all,
  // deselect, open, about already match.
};

/** Build the action map keyed by *registry* command id (Rust side). For
 * each `useAppCommands` entry, we also index its TS id under whichever
 * Rust id aliases to it, so a registry hit on `"union"` dispatches the
 * closure whose TS id is `"boolean-union"`. */
function buildActionMap(
  commands: ReturnType<typeof useAppCommands>,
): Map<string, () => void> {
  const byTsId = new Map<string, () => void>();
  for (const cmd of commands) {
    byTsId.set(cmd.id, cmd.action);
  }
  const out = new Map<string, () => void>();
  // Direct hits — any TS id that is also a registry id.
  for (const [tsId, action] of byTsId) {
    out.set(tsId, action);
  }
  // Aliased hits — for each registry id that aliases to a TS id, register
  // the action under the registry id too.
  for (const [rustId, tsId] of Object.entries(ID_ALIASES)) {
    const action = byTsId.get(tsId);
    if (action) out.set(rustId, action);
  }
  return out;
}

/** Read stores once per dispatch and produce the current [`WhenContext`]
 * flag set. Called inside the keydown handler so state is always fresh. */
function readWhenContext(target: EventTarget | null): number {
  const inputFocused = isInputFocused(target);
  const ui = useUiStore.getState();
  const doc = useDocumentStore.getState();
  const sketch = useSketchStore.getState();
  const sim = useSimulationStore.getState();
  const electronics = useElectronicsStore.getState();

  // canUndo / canRedo live on the engine, not the store — for now leave
  // them off and let useKeyboardShortcuts handle Ctrl+Z / Ctrl+Shift+Z.
  // They'll get wired up in the follow-up slice once the store exposes
  // the flags as selectors.
  return buildWhenContext({
    inputFocused,
    menuOpen: false,
    commandMode: false,
    selectionSize: ui.selectedPartIds.size,
    partCount: doc.parts.length,
    canUndo: false,
    canRedo: false,
    sketchHasPoints: sketch.active && sketch.segments.length > 0,
    physicsRunning: sim.mode === "running",
    electronicsActive: electronics.active ?? false,
  });
}

/** Work out which [`AppMode`] the registry should filter against. */
function readCurrentMode(): AppMode {
  if (useSketchStore.getState().active) return "Sketch";
  if (useElectronicsStore.getState().active) return "Electronics";
  if (useSimulationStore.getState().mode === "running") return "Physics";
  return "Normal";
}

export function useKeybindingDispatcher({
  onAboutOpen,
  onSave,
  onOpen,
}: UseKeybindingDispatcherProps): void {
  // The registry is loaded lazily from wasm; hold a ref so the keydown
  // handler can see the current value without re-binding on every mount.
  const registryRef = useRef<KeybindingRegistry | null>(null);

  useEffect(() => {
    let cancelled = false;
    getKeybindingRegistry()
      .then((reg) => {
        if (!cancelled) registryRef.current = reg;
      })
      .catch((err) => {
        console.error("[keybindings] failed to load registry:", err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Action map is rebuilt whenever useAppCommands' memoized list changes
  // (rarely — it's memoized against a small set of callbacks).
  const commands = useAppCommands({
    onDismiss: () => {
      /* no-op: dispatcher doesn't own any UI state to dismiss */
    },
    onAboutOpen,
    onSave,
    onOpen,
    surface: "desktop-menu",
  });
  const actionMap = useMemo(() => buildActionMap(commands), [commands]);
  const actionMapRef = useRef(actionMap);
  actionMapRef.current = actionMap;

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const registry = registryRef.current;
      if (!registry) return;

      const chord = chordFromEvent(e);
      if (!chord) return;

      const mode = readCurrentMode();
      const ctxBits = readWhenContext(e.target);
      const id = registry.resolve(chord, mode, ctxBits);
      if (!id) return;

      const action = actionMapRef.current.get(id);
      if (!action) return;

      e.preventDefault();
      e.stopPropagation();
      action();
    };

    // Capture phase so we run before bubble-phase handlers on nested
    // elements (and before the legacy useKeyboardShortcuts handler which
    // attaches without capture).
    window.addEventListener("keydown", handler, { capture: true });
    return () => {
      window.removeEventListener("keydown", handler, { capture: true });
    };
  }, []);
}
