import "./lib/crypto-polyfill";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { initLocale } from "@/stores/locale-store";

initLocale();

// Unregister service workers in dev mode to prevent stale cache issues
if (import.meta.env.DEV && "serviceWorker" in navigator) {
  navigator.serviceWorker.getRegistrations().then((registrations) => {
    for (const registration of registrations) {
      registration.unregister();
      console.log("[dev] Unregistered stale service worker");
    }
  });
}
import {
  AuthProvider,
  configureStorage,
  configureVersionHistoryStorage,
  initSyncListeners,
  triggerSync,
  type StorageAdapter,
} from "@vcad/auth";
import { App } from "./App";
import { CliAuth } from "./components/CliAuth";
import "./index.css";
import {
  getAllDocuments,
  loadDocument,
  saveCompleteDocument,
  updateDocument,
} from "./lib/storage";
import { installLastOpenedTracker } from "./lib/last-opened";
import { useBootStore } from "./stores/boot-store";

// Configure storage adapter for auth/sync
const storageAdapter: StorageAdapter = {
  getAllDocuments: async () => {
    const docs = await getAllDocuments();
    return docs.map((d) => ({
      id: d.id,
      name: d.name,
      document: d.document,
      createdAt: d.createdAt,
      modifiedAt: d.modifiedAt,
      version: d.version,
      syncStatus: d.syncStatus,
      cloudId: d.cloudId,
      thumbnail: d.thumbnail,
    }));
  },
  getDocument: async (id) => {
    const doc = await loadDocument(id);
    if (!doc) return null;
    return {
      id: doc.id,
      name: doc.name,
      document: doc.document,
      createdAt: doc.createdAt,
      modifiedAt: doc.modifiedAt,
      version: doc.version,
      syncStatus: doc.syncStatus,
      cloudId: doc.cloudId,
      thumbnail: doc.thumbnail,
    };
  },
  saveDocument: async (doc) => {
    await saveCompleteDocument({
      id: doc.id,
      name: doc.name,
      document: doc.document as import("@vcad/core").VcadFile,
      createdAt: doc.createdAt,
      modifiedAt: doc.modifiedAt,
      version: doc.version,
      syncStatus: doc.syncStatus,
      cloudId: doc.cloudId,
      thumbnail: doc.thumbnail,
    });
  },
  updateDocument: async (id, updates) => {
    await updateDocument(id, updates as Parameters<typeof updateDocument>[1]);
  },
};

configureStorage(storageAdapter);
configureVersionHistoryStorage(storageAdapter);
initSyncListeners();
installLastOpenedTracker();

/**
 * Defer the first sync until bootstrap finishes.
 *
 * On cold-start, AuthProvider's `ensureSession()` resolves quickly with the
 * cached session and fires `onSignIn` while `bootstrap()` is still resolving
 * the document. The original handler called `triggerSync()` immediately,
 * which raced our IDB read in `fetchDocumentData()` — sync's downloads could
 * land first and shift "most recent" out from under us. Gating the first
 * sync on `phase === "ready"` removes that race entirely. Later sign-ins
 * (user clicks "Sign in") happen long after boot, so they're unaffected.
 */
let firstSyncDone = false;
function onSignInGated(): void {
  if (firstSyncDone) {
    void triggerSync();
    return;
  }
  if (useBootStore.getState().phase === "ready") {
    firstSyncDone = true;
    void triggerSync();
    return;
  }
  const unsub = useBootStore.subscribe((s) => {
    if (s.phase !== "ready") return;
    unsub();
    firstSyncDone = true;
    void triggerSync();
  });
}

// Route: `/cli-auth` is the device-code browser flow completion page for
// `vcad login`. Check the pathname at mount time and render the standalone
// CliAuth component instead of the full editor — zero risk of breaking the
// normal load path since App never runs in this branch.
const isCliAuth = window.location.pathname === "/cli-auth";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AuthProvider onSignIn={onSignInGated}>
      {isCliAuth ? <CliAuth /> : <App />}
    </AuthProvider>
  </StrictMode>,
);

// Tauri desktop: close the native splashscreen window and reveal the main
// window once the React tree has painted. The in-app <Splash> takes over
// from here for the rest of bootstrap.
requestAnimationFrame(() => {
  void (async () => {
    try {
      const { isTauri, invoke } = await import("@/lib/tauri");
      if (isTauri()) await invoke("close_splashscreen");
    } catch {
      // Splash close is best-effort — if it fails the user just sees both
      // windows briefly. Never block startup on it.
    }
  })();
});

// Vercel Analytics + Speed Insights — prod only, silently skip if blocked
if (import.meta.env.PROD) {
  import("@vercel/analytics").then(({ inject }) => inject()).catch(() => {});
  import("@vercel/speed-insights").then(({ injectSpeedInsights }) => injectSpeedInsights()).catch(() => {});
}

// Defer analytics until after first paint
const initAnalytics = () => {
  const posthogKey = import.meta.env.VITE_POSTHOG_KEY;
  const posthogHost =
    import.meta.env.VITE_POSTHOG_HOST || "https://us.i.posthog.com";
  if (posthogKey) {
    import("posthog-js").then(({ default: posthog }) => {
      posthog.init(posthogKey, {
        api_host: posthogHost,
        person_profiles: "identified_only",
        capture_pageview: true,
        capture_pageleave: true,
        session_recording: {
          maskAllInputs: false,
          maskInputOptions: { password: true },
        },
      });
    });
  }
};
if ("requestIdleCallback" in window) {
  requestIdleCallback(initAnalytics);
} else {
  setTimeout(initAnalytics, 1000);
}
