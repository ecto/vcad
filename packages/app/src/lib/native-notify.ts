/**
 * Native (Tauri) notifications.
 *
 * Exposes a single `notify({ title, body })` helper that routes to the OS
 * notification center when running under Tauri — and is a no-op in the
 * browser build. The permission prompt is handled transparently on first
 * use; subsequent calls skip the roundtrip.
 */

import { isTauri } from "@/lib/tauri";

interface NotifyOptions {
  title: string;
  body?: string;
}

let permissionGranted: boolean | null = null;
let permissionInflight: Promise<boolean> | null = null;

async function ensurePermission(): Promise<boolean> {
  if (permissionGranted !== null) return permissionGranted;
  if (permissionInflight) return permissionInflight;
  permissionInflight = (async () => {
    try {
      const mod = await import("@tauri-apps/plugin-notification");
      let granted = await mod.isPermissionGranted();
      if (!granted) {
        const res = await mod.requestPermission();
        granted = res === "granted";
      }
      permissionGranted = granted;
      return granted;
    } catch {
      permissionGranted = false;
      return false;
    } finally {
      permissionInflight = null;
    }
  })();
  return permissionInflight;
}

/** Fire a native notification. Silently no-ops outside Tauri. */
export async function notify(opts: NotifyOptions): Promise<void> {
  if (!isTauri()) return;
  try {
    const granted = await ensurePermission();
    if (!granted) return;
    const mod = await import("@tauri-apps/plugin-notification");
    mod.sendNotification({ title: opts.title, body: opts.body });
  } catch (err) {
    // Don't let notification failures surface to the user — they're advisory.
    console.warn("[notify] native notification failed:", err);
  }
}

/**
 * Fire a native notification only when the app is backgrounded. Useful for
 * long-running work (exports, AI, slicer) where the user likely switched
 * away — in-app toasts stay for foreground, OS notifications cover
 * background.
 */
export async function notifyIfBackgrounded(opts: NotifyOptions): Promise<void> {
  if (!isTauri()) return;
  if (typeof document !== "undefined" && document.visibilityState === "visible") {
    return;
  }
  await notify(opts);
}
