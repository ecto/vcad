import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

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

// Route: `/cli-auth` is the device-code browser flow completion page for
// `vcad login`. Check the pathname at mount time and render the standalone
// CliAuth component instead of the full editor — zero risk of breaking the
// normal load path since App never runs in this branch.
const isCliAuth = window.location.pathname === "/cli-auth";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <AuthProvider onSignIn={() => triggerSync()}>
      {isCliAuth ? <CliAuth /> : <App />}
    </AuthProvider>
  </StrictMode>,
);

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
