/**
 * Native macOS context menus via the Tauri menu bridge.
 *
 * The Rust side (`crates/vcad-desktop/src/commands/context_menu.rs`) takes
 * a flat menu spec, builds a real `NSMenu` (or GTK / Win32 equivalent on
 * other desktops), pops it under the cursor, and emits a
 * `context-menu-select` event with the chosen id when the user picks
 * something. This module is the JS counterpart: a small `popup()` helper
 * that returns a Promise resolving to the chosen id (or null on dismiss).
 *
 * In the browser (or any non-Tauri environment) the helper rejects the
 * promise with a sentinel — callers fall back to the Radix-rendered HTML
 * menu in that case. On Linux/Windows the OS menu renders normally; we
 * keep the contract identical because GTK/Win32 menus look closer to
 * "native" than our HTML clone too.
 */

import { invoke, isTauri } from "@/lib/tauri";

export interface NativeMenuLeaf {
  kind: "item";
  id: string;
  label: string;
  accelerator?: string;
  disabled?: boolean;
  /** Renders a leading checkmark — for radio-group / toggle items. */
  checked?: boolean;
}

export interface NativeMenuSeparator {
  kind: "separator";
}

export interface NativeMenuSubmenu {
  kind: "submenu";
  label: string;
  items: NativeMenuItem[];
}

export type NativeMenuItem =
  | NativeMenuLeaf
  | NativeMenuSeparator
  | NativeMenuSubmenu;

/** Available only inside Tauri; cheap predicate so callers can branch
 * without async work in render. */
export function nativeMenuAvailable(): boolean {
  return isTauri();
}

/**
 * Pop a native context menu and resolve to the chosen item id, or `null`
 * if the user dismissed it (clicked away / pressed Escape).
 *
 * The Rust side is fire-and-forget — there's no callback when the menu
 * dismisses without a selection — so we listen for the next select event
 * and race it against a short watchdog. Once we see a click or 30s pass
 * we stop listening to avoid leaks. 30s is generous: real users either
 * pick within ~2s or move the mouse and dismiss; the timeout is just a
 * safety net so menu-listener subscriptions can't accumulate.
 */
export async function popupNativeContextMenu(
  items: NativeMenuItem[],
): Promise<string | null> {
  if (!isTauri()) {
    throw new Error("native context menu unavailable");
  }
  const { listen } = await import("@tauri-apps/api/event");
  let unlisten: { fn: (() => void) | null } = { fn: null };

  const choice = new Promise<string | null>((resolve) => {
    let settled = false;
    listen<{ id: string }>("context-menu-select", (e) => {
      if (settled) return;
      settled = true;
      resolve(e.payload.id);
    }).then((u) => {
      unlisten.fn = u;
      // Watchdog — if no click in 30s the user dismissed; stop waiting.
      window.setTimeout(() => {
        if (settled) return;
        settled = true;
        resolve(null);
      }, 30000);
    });
  });

  await invoke<void>("show_context_menu", { items });
  const id = await choice;
  unlisten.fn?.();
  return id;
}
