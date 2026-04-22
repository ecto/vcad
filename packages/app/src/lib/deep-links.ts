/**
 * Deep link bridge (`vcad://...`).
 *
 * Registers a listener for incoming URLs and dispatches them as
 * `vcad:deep-link` custom events. Feature code subscribes rather than
 * hitting the Tauri plugin directly — same pattern as the menu bridge.
 *
 * Example URLs:
 *   vcad://open/<file-id>        — open a shared document
 *   vcad://model/<slug>          — jump to a catalog model
 */

import { isTauri } from "@/lib/tauri";

let installed = false;

export async function installDeepLinkListener(): Promise<void> {
  if (installed) return;
  installed = true;
  if (!isTauri()) return;
  try {
    const { onOpenUrl } = await import("@tauri-apps/plugin-deep-link");
    await onOpenUrl((urls) => {
      for (const url of urls) {
        window.dispatchEvent(
          new CustomEvent("vcad:deep-link", { detail: { url } }),
        );
      }
    });
  } catch (err) {
    console.warn("[deep-link] setup failed:", err);
  }
}
