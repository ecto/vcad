import { useRegisterSW } from "virtual:pwa-register/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "@/lib/tauri";

// Piggyback on the desktop updater manifest: `.github/workflows/desktop-release.yml`
// only publishes this file on `v*` tag pushes, so gating the banner on it keeps
// ad-hoc main-branch deploys from surfacing as "Update Available" to web users.
const LATEST_JSON_URL =
  "https://github.com/ecto/vcad/releases/latest/download/latest.json";
const CHECK_INTERVAL_MS = 60 * 60 * 1000;

function isNewerVersion(remote: string, local: string): boolean {
  const parse = (s: string): number[] =>
    s
      .replace(/^v/, "")
      .split(/[.+-]/)
      .map((p) => parseInt(p, 10) || 0);
  const r = parse(remote);
  const l = parse(local);
  const len = Math.max(r.length, l.length);
  for (let i = 0; i < len; i++) {
    const a = r[i] ?? 0;
    const b = l[i] ?? 0;
    if (a > b) return true;
    if (a < b) return false;
  }
  return false;
}

export function useAppUpdate() {
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const [latestTagVersion, setLatestTagVersion] = useState<string | null>(null);
  const [dismissedVersion, setDismissedVersion] = useState<string | null>(null);

  const {
    offlineReady: [offlineReady, setOfflineReady],
    updateServiceWorker,
  } = useRegisterSW({
    onRegisteredSW(_, r) {
      if (!r) return;
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
      }
      intervalRef.current = setInterval(() => r.update(), CHECK_INTERVAL_MS);
    },
  });

  useEffect(() => {
    // Tauri has its own signed updater (see native-updater.ts) that hits the
    // same manifest — skip the web path inside the desktop shell.
    if (isTauri()) return;

    let cancelled = false;
    const check = async () => {
      try {
        const res = await fetch(LATEST_JSON_URL, { cache: "no-store" });
        if (!res.ok) return;
        const data = (await res.json()) as { version?: string };
        if (!cancelled && typeof data.version === "string") {
          setLatestTagVersion(data.version);
        }
      } catch {
        // Offline, rate-limited, CORS hiccup — retry on next interval.
      }
    };
    void check();
    const id = setInterval(check, CHECK_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  useEffect(() => {
    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
  }, []);

  const updateAvailable =
    latestTagVersion !== null &&
    latestTagVersion !== dismissedVersion &&
    isNewerVersion(latestTagVersion, __APP_VERSION__);

  return {
    updateAvailable,
    offlineReady,
    applyUpdate: useCallback(
      () => updateServiceWorker(true),
      [updateServiceWorker]
    ),
    dismissUpdate: useCallback(
      () => setDismissedVersion(latestTagVersion),
      [latestTagVersion]
    ),
    dismissOfflineReady: useCallback(
      () => setOfflineReady(false),
      [setOfflineReady]
    ),
    version: __APP_VERSION__,
    buildTime: __BUILD_TIME__,
    latestTagVersion,
  };
}
