/**
 * Tauri desktop integration layer.
 *
 * Thin wrapper around `@tauri-apps/api/core`: exposes environment
 * detection and a typed `invoke` helper. Feature-specific command
 * wrappers live next to the feature code (see e.g. `print-relay.ts`).
 */

import { invoke as tauriInvoke, isTauri as tauriIsTauri } from "@tauri-apps/api/core";

export function isTauri(): boolean {
  return tauriIsTauri();
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error(`Tauri not available (attempted invoke: ${cmd})`);
  }
  return tauriInvoke<T>(cmd, args);
}
