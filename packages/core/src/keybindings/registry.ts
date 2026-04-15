/**
 * TS-side wrapper around the wasm `KeybindingRegistry`.
 *
 * Owns the lifetime of the underlying `WasmKeybindings` instance, typed
 * accessors, and in-memory caching of the command list so the UI isn't
 * forced to JSON-parse on every render.
 */

import type { Chord } from "./chord.js";
import type { WhenBits } from "./when-context.js";
import { getKernelWasm, getKernelWasmSync } from "../wasm-singleton.js";

/** Mode name mirror of `vcad_app::mode::AppMode`. The wasm side accepts the
 * string form; mode-scope filtering on the Rust side maps it via `parse`. */
export type AppMode =
  | "Normal"
  | "Sketch"
  | "Assembly"
  | "Physics"
  | "Cam"
  | "Print"
  | "Electronics"
  | "Drawing";

/** JSON shape returned by `WasmKeybindings::commandsJson`, one per registered
 * command. `effective_chord` already folds in any user override. */
export interface CommandView {
  id: string;
  label: string;
  keywords: readonly string[];
  icon: string;
  category: string | null;
  default_chord: Chord | null;
  effective_chord: Chord | null;
  when: string | null;
  mode_scope:
    | { kind: "Global" }
    | { kind: "Mode"; modes: string }
    | { kind: "Modes"; modes: string[] };
  target: "kernel" | "host";
}

type WasmKeybindingsInstance = {
  resolve(chordJson: string, mode: string, ctxBits: number): string | undefined;
  commandsJson(): string;
  setBinding(id: string, chordJson?: string | null): void;
  chordFor(id: string): string | undefined;
  resetAll(): void;
  saveOverrides(): string;
  loadOverrides(json: string): boolean;
  conflictsJson(mode: string): string;
  free?(): void;
};

let singleton: KeybindingRegistry | null = null;

/** Get the shared registry, initializing it from the wasm module if needed. */
export async function getKeybindingRegistry(): Promise<KeybindingRegistry> {
  if (singleton) return singleton;
  const wasm = await getKernelWasm();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const WasmKeybindings = (wasm as any).WasmKeybindings;
  if (!WasmKeybindings) {
    throw new Error("WasmKeybindings not found in kernel-wasm module");
  }
  singleton = new KeybindingRegistry(new WasmKeybindings() as WasmKeybindingsInstance);
  return singleton;
}

/** Synchronous accessor — returns `null` if the registry hasn't been
 * initialized yet. Useful inside hooks that can't await. */
export function getKeybindingRegistrySync(): KeybindingRegistry | null {
  if (singleton) return singleton;
  const wasm = getKernelWasmSync();
  if (!wasm) return null;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const WasmKeybindings = (wasm as any).WasmKeybindings;
  if (!WasmKeybindings) return null;
  singleton = new KeybindingRegistry(new WasmKeybindings() as WasmKeybindingsInstance);
  return singleton;
}

/** Wrapper around the wasm-side registry. Caches the commands list since
 * it's queried repeatedly by menus and the palette. */
export class KeybindingRegistry {
  private inner: WasmKeybindingsInstance;
  private cachedCommands: readonly CommandView[] | null = null;

  constructor(inner: WasmKeybindingsInstance) {
    this.inner = inner;
  }

  /** Resolve a chord to a command id, or `null` if nothing binds. */
  resolve(chord: Chord, mode: AppMode, ctxBits: WhenBits): string | null {
    const json = JSON.stringify(chord);
    const id = this.inner.resolve(json, mode, ctxBits);
    return id ?? null;
  }

  /** Full command list with current effective chords. Cached — call
   * `invalidate` after a rebind if you need a fresh copy. */
  commands(): readonly CommandView[] {
    if (!this.cachedCommands) {
      this.cachedCommands = JSON.parse(this.inner.commandsJson()) as CommandView[];
    }
    return this.cachedCommands;
  }

  /** The effective chord (user override, or default) for a command id. */
  chordFor(id: string): Chord | null {
    const s = this.inner.chordFor(id);
    if (!s) return null;
    return JSON.parse(s) as Chord;
  }

  /** Rebind or clear. Pass `null` to clear (disable) the binding. */
  setBinding(id: string, chord: Chord | null): void {
    this.inner.setBinding(id, chord ? JSON.stringify(chord) : undefined);
    this.cachedCommands = null;
  }

  /** Clear all user overrides, restoring defaults. */
  resetAll(): void {
    this.inner.resetAll();
    this.cachedCommands = null;
  }

  /** Serialize overrides for localStorage. */
  saveOverrides(): string {
    return this.inner.saveOverrides();
  }

  /** Load overrides previously returned by `saveOverrides`. */
  loadOverrides(json: string): boolean {
    const ok = this.inner.loadOverrides(json);
    this.cachedCommands = null;
    return ok;
  }

  /** Binding conflicts in the given mode (two commands sharing a chord). */
  conflicts(mode: AppMode): Array<{ chord: Chord; ids: string[] }> {
    return JSON.parse(this.inner.conflictsJson(mode)) as Array<{
      chord: Chord;
      ids: string[];
    }>;
  }

  /** Drop the cached command list — call after a rebind or mode change. */
  invalidate(): void {
    this.cachedCommands = null;
  }
}
