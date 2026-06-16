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

/** Second registration of the same HTML for ChatGPT (OpenAI Apps SDK):
 *  referenced from tools via `_meta["openai/outputTemplate"]` and served
 *  with the skybridge MIME type so ChatGPT injects its widget bridge
 *  (the viewer detects `window.openai` and adapts — see
 *  viewer-app/openai-shim.ts). */
export const OPENAI_VIEWER_RESOURCE_URI = "ui://vcad/viewer-openai.html";

/** MIME type marking a resource as a ChatGPT Apps SDK widget. */
export const OPENAI_APP_MIME_TYPE = "text/html+skybridge";

/** ChatGPT widget CSP: the bundle is fully inlined; blob: is needed for
 *  GLTFLoader's createObjectURL textures. */
export const OPENAI_WIDGET_CSP = {
  connect_domains: [] as string[],
  resource_domains: ["blob:"],
};

/** Return the self-contained viewer HTML. */
export function getViewerHtml(): string {
  return VIEWER_HTML;
}
