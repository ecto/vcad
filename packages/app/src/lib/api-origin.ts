/**
 * API origin resolver.
 *
 * Browser builds hit same-origin `/api/*` routes — Vercel functions in prod,
 * vite devApiPlugin in dev. Tauri builds load from a `tauri://` webview
 * origin where those routes don't exist, so we redirect them to the
 * production API. Set `VITE_API_ORIGIN` at build time to point at a
 * staging environment instead.
 */

import { isTauri } from "@/lib/tauri";

const BUILD_API_ORIGIN =
  typeof import.meta !== "undefined" &&
  (import.meta as { env?: Record<string, string> }).env?.VITE_API_ORIGIN;

/** Production origin for the vcad API. Matches the deployed web app. */
const DEFAULT_TAURI_API_ORIGIN = "https://vcad.io";

/**
 * Rewrite a path-relative API URL (`/api/foo`) into an absolute URL when we
 * need to escape the Tauri webview origin. Absolute URLs pass through
 * unchanged.
 */
export function apiUrl(path: string): string {
  if (/^https?:\/\//.test(path)) return path;
  if (!isTauri()) return path;
  const origin = BUILD_API_ORIGIN || DEFAULT_TAURI_API_ORIGIN;
  return `${origin.replace(/\/$/, "")}${path.startsWith("/") ? path : `/${path}`}`;
}
