/**
 * Web popup-window OAuth flow.
 *
 * Opens a same-origin popup, drives the provider redirect inside it,
 * and hands the resulting callback URL back to the main window via
 * `postMessage`. The main window completes the PKCE exchange against
 * the existing Supabase client (the code verifier is in shared
 * `localStorage`) so `onAuthStateChange` fires in the original tab and
 * the user keeps their place — no navigation, no state loss.
 *
 * Tauri uses a separate flow (external browser + deep-link bridge); the
 * caller should branch on `isTauriRuntime()` before calling this.
 */

import { applyAuthCallback } from "./auth-deep-link";
import { getPopupCallbackUrl, getSupabase } from "./client";

export type OAuthPopupProvider = "google" | "github";

export interface OAuthPopupResult {
  ok: boolean;
  /**
   * True when the popup was blocked or unavailable and the caller should
   * fall back to a full-page redirect. `error` is unset in this case.
   */
  popupBlocked?: boolean;
  /** True when the user closed the popup before completing sign-in. */
  cancelled?: boolean;
  /** Populated on failure; safe to surface to the user. */
  error?: string;
}

/**
 * postMessage payload posted by `/auth/popup` once the OAuth provider
 * has redirected to it. The main window is the only intended consumer.
 */
interface OAuthCallbackMessage {
  type: "vcad:oauth-callback";
  url: string;
}

function isCallbackMessage(data: unknown): data is OAuthCallbackMessage {
  if (typeof data !== "object" || data === null) return false;
  const msg = data as Record<string, unknown>;
  return (
    msg.type === "vcad:oauth-callback" && typeof msg.url === "string"
  );
}

const POPUP_NAME = "vcad-oauth";
const POPUP_FEATURES =
  "popup=yes,width=500,height=700,menubar=no,toolbar=no,location=no,status=no";

/**
 * Sign in with an OAuth provider via a popup window. Must be called
 * synchronously from a user-gesture handler (click) or the popup will
 * be blocked. Returns `{ popupBlocked: true }` when the browser refused
 * to open the popup so the caller can fall back to a full-page
 * redirect.
 */
export async function signInWithOAuthPopup(
  provider: OAuthPopupProvider,
): Promise<OAuthPopupResult> {
  if (typeof window === "undefined") {
    return { ok: false, error: "Popup OAuth requires a browser" };
  }

  const supabase = getSupabase();
  if (!supabase) {
    return { ok: false, error: "Auth is not configured" };
  }

  // Open synchronously inside the user gesture; assigning `location`
  // after the async `signInWithOAuth` round-trip is fine.
  const popup = window.open("about:blank", POPUP_NAME, POPUP_FEATURES);
  if (!popup) {
    return { ok: false, popupBlocked: true };
  }

  try {
    popup.document.write(
      '<!doctype html><meta charset="utf-8"><title>Connecting…</title>' +
        '<body style="background:#222;color:#F8F8F2;font:12px ui-monospace,monospace;display:flex;align-items:center;justify-content:center;height:100vh;margin:0">Connecting…</body>',
    );
  } catch {
    // Some browsers throw on document.write into about:blank when
    // tracking-protection rules apply. Harmless — the popup just
    // stays blank until we navigate it below.
  }

  const { data, error } = await supabase.auth.signInWithOAuth({
    provider,
    options: {
      redirectTo: getPopupCallbackUrl(),
      skipBrowserRedirect: true,
    },
  });

  if (error || !data?.url) {
    popup.close();
    return {
      ok: false,
      error: error?.message ?? "OAuth provider did not return a redirect URL",
    };
  }

  popup.location.href = data.url;

  return new Promise<OAuthPopupResult>((resolve) => {
    const expectedOrigin = window.location.origin;
    let settled = false;

    const cleanup = () => {
      window.removeEventListener("message", onMessage);
      if (pollHandle !== undefined) clearInterval(pollHandle);
      try {
        if (!popup.closed) popup.close();
      } catch {
        // Cross-origin access on `closed` can throw briefly while the
        // provider page is loading; safe to ignore on cleanup.
      }
    };

    const onMessage = async (event: MessageEvent) => {
      if (event.origin !== expectedOrigin) return;
      if (event.source !== popup) return;
      if (!isCallbackMessage(event.data)) return;

      if (settled) return;
      settled = true;

      const result = await applyAuthCallback(event.data.url);
      cleanup();
      if (result.ok) {
        resolve({ ok: true });
      } else {
        resolve({ ok: false, error: result.error });
      }
    };

    window.addEventListener("message", onMessage);

    const pollHandle = window.setInterval(() => {
      let closed = false;
      try {
        closed = popup.closed;
      } catch {
        // Ignore — see comment in cleanup().
      }
      if (closed && !settled) {
        settled = true;
        cleanup();
        resolve({ ok: false, cancelled: true });
      }
    }, 500);
  });
}
