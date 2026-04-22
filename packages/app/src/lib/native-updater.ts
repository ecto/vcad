/**
 * Tauri auto-updater bridge.
 *
 * Production distribution should run `tauri signer generate` once, commit
 * the public key into tauri.conf.json under `plugins.updater.pubkey`, and
 * publish a signed `latest.json` at each release. Until then these hooks
 * will report "no update available" because there's no endpoint to query.
 */

import { isTauri } from "@/lib/tauri";
import { useNotificationStore } from "@/stores/notification-store";

interface CheckOptions {
  /** When true, notify the user even if no update is available. Used for
   * the explicit "Check for Updates" menu item; startup checks stay quiet. */
  announceIfUpToDate?: boolean;
}

let startupCheckRan = false;

/**
 * Check for updates. Prompts the user on success; silently no-ops if the
 * updater isn't configured or we're in the browser.
 */
export async function checkForUpdates(opts: CheckOptions = {}): Promise<void> {
  if (!isTauri()) return;
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (!update) {
      if (opts.announceIfUpToDate) {
        useNotificationStore
          .getState()
          .addToast("vcad is up to date.", "info");
      }
      return;
    }
    const toast = useNotificationStore.getState();
    toast.addToast(
      `Update available: v${update.version}. Downloading…`,
      "info",
      8000,
    );
    await update.downloadAndInstall();
    // On macOS/Linux the app needs a relaunch to swap the binary.
    toast.addToast("Update installed. Restart vcad to apply.", "success", 0);
  } catch (err) {
    console.warn("[updater] check failed:", err);
    if (opts.announceIfUpToDate) {
      useNotificationStore
        .getState()
        .addToast(
          "Couldn't check for updates. Check your connection.",
          "error",
        );
    }
  }
}

/** Run a silent update check once per session, a few seconds after launch. */
export function scheduleStartupUpdateCheck() {
  if (startupCheckRan) return;
  startupCheckRan = true;
  if (!isTauri()) return;
  setTimeout(() => {
    void checkForUpdates();
  }, 5_000);
}
