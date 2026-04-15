/**
 * Hook that loads the shared `KeybindingRegistry`, persists user overrides
 * to localStorage, and exposes a small reactive API for the prefs UI.
 *
 * The registry itself is a singleton owned by `@vcad/core` — this hook
 * just bumps a version counter when the user rebinds so React knows to
 * re-render the panel.
 */

import { useEffect, useState, useCallback, useMemo } from "react";
import {
  getKeybindingRegistry,
  type KeybindingRegistry,
  type Chord,
  type KeybindingCommandView,
  type KeybindingMode,
} from "@vcad/core";

const STORAGE_KEY = "vcad.keybinding.overrides";

/** Small stable snapshot the prefs UI consumes. */
export interface KeybindingPrefs {
  /** Loaded registry, or `null` while the wasm module is initializing. */
  registry: KeybindingRegistry | null;
  /** All commands with their effective chords. Refreshed on every rebind. */
  commands: readonly KeybindingCommandView[];
  /** Conflict pairs in the given mode. */
  conflicts: ReturnType<KeybindingRegistry["conflicts"]>;
  /** Rebind a command and persist. Pass `null` to clear. */
  setBinding: (id: string, chord: Chord | null) => void;
  /** Restore all defaults. */
  resetAll: () => void;
}

export function useKeybindingPrefs(mode: KeybindingMode = "Normal"): KeybindingPrefs {
  const [registry, setRegistry] = useState<KeybindingRegistry | null>(null);
  const [version, setVersion] = useState(0);

  // Load registry + persisted overrides once.
  useEffect(() => {
    let cancelled = false;
    getKeybindingRegistry()
      .then((reg) => {
        if (cancelled) return;
        try {
          const json = localStorage.getItem(STORAGE_KEY);
          if (json) reg.loadOverrides(json);
        } catch (err) {
          console.warn("[keybindings] failed to load overrides:", err);
        }
        setRegistry(reg);
      })
      .catch((err) => {
        console.error("[keybindings] failed to load registry:", err);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const setBinding = useCallback(
    (id: string, chord: Chord | null) => {
      if (!registry) return;
      registry.setBinding(id, chord);
      try {
        localStorage.setItem(STORAGE_KEY, registry.saveOverrides());
      } catch (err) {
        console.warn("[keybindings] failed to persist override:", err);
      }
      setVersion((v) => v + 1);
    },
    [registry],
  );

  const resetAll = useCallback(() => {
    if (!registry) return;
    registry.resetAll();
    try {
      localStorage.removeItem(STORAGE_KEY);
    } catch {
      // ignore
    }
    setVersion((v) => v + 1);
  }, [registry]);

  // Snapshots — recomputed when version bumps.
  const commands = useMemo(() => {
    if (!registry) return [];
    return registry.commands();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [registry, version]);

  const conflicts = useMemo(() => {
    if (!registry) return [];
    return registry.conflicts(mode);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [registry, mode, version]);

  return { registry, commands, conflicts, setBinding, resetAll };
}
