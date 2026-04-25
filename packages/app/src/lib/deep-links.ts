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
 *   vcad://auth/callback?...     — completes a desktop magic-link sign-in
 *
 * Auth callbacks are handled inline (the @vcad/auth package owns the
 * Supabase exchange) before the event fan-out, since the rest of the
 * app should never see auth tokens floating past as generic deep links.
 */

import { handleAuthDeepLink, isAuthDeepLink } from "@vcad/auth";

import { isTauri } from "@/lib/tauri";

let installed = false;

async function dispatchDeepLink(url: string): Promise<void> {
  if (isAuthDeepLink(url)) {
    const result = await handleAuthDeepLink(url);
    if (!result.ok) {
      console.warn("[deep-link] auth callback failed:", result.error);
      window.dispatchEvent(
        new CustomEvent("vcad:auth-callback-failed", {
          detail: { error: result.error },
        }),
      );
    }
    return;
  }
  window.dispatchEvent(
    new CustomEvent("vcad:deep-link", { detail: { url } }),
  );
}

export async function installDeepLinkListener(): Promise<void> {
  if (installed) return;
  installed = true;
  if (!isTauri()) return;
  try {
    const { onOpenUrl } = await import("@tauri-apps/plugin-deep-link");
    await onOpenUrl((urls) => {
      for (const url of urls) {
        void dispatchDeepLink(url);
      }
    });
  } catch (err) {
    console.warn("[deep-link] setup failed:", err);
  }
}
