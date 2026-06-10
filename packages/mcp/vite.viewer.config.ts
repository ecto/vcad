import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// Bundles viewer-app/ into a single self-contained HTML file (Three.js and
// the ext-apps SDK inlined) — the sandboxed iframe cannot rely on CDN
// availability, and singlefile output is the MCP Apps recommended shape.
// Named vite.viewer.config.ts (not vite.config.ts) so vitest doesn't pick
// up the viewer-app root for test discovery.
export default defineConfig({
  root: "viewer-app",
  plugins: [viteSingleFile()],
  build: {
    outDir: "../dist-viewer",
    emptyOutDir: true,
  },
});
