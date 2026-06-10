/**
 * MCP Apps viewer resource for inline 3D CAD preview.
 *
 * The viewer HTML is built by Vite from `viewer-app/` into a single
 * self-contained file (Three.js + the official `@modelcontextprotocol/ext-apps`
 * App class inlined) and embedded at build time via
 * `scripts/wrap-viewer.mjs`. Host communication follows SEP-1865: the App
 * class owns the `ui/initialize` → `ui/notifications/initialized` handshake,
 * and the view fetches geometry through the app-only `get_preview_glb` tool.
 */

import { VIEWER_HTML } from "./viewer-html.generated.js";

/**
 * CSP for the viewer. Everything is inlined, so no external domains.
 * `blob:` is allowed because GLTFLoader uses createObjectURL for embedded
 * textures; hosts like Cursor enforce the declared CSP strictly (tldraw's
 * working app whitelists blob: for the same reason).
 */
export const VIEWER_CSP: { resourceDomains: string[] } = {
  resourceDomains: ["blob:"],
};

/** The ui:// URI for the viewer resource. */
export const VIEWER_RESOURCE_URI = "ui://vcad/viewer";

/** MIME type for MCP App HTML resources. */
export const MCP_APP_MIME_TYPE = "text/html;profile=mcp-app";

/** Return the self-contained viewer HTML. */
export function getViewerHtml(): string {
  return VIEWER_HTML;
}
