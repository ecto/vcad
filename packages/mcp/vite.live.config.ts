import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// Bundles live-app/ into one self-contained HTML file (Three.js +
// @supabase/supabase-js inlined). Served by the MCP server at GET /live/<id>
// for shared sessions. Mirrors vite.viewer.config.ts; separate config name so
// vitest doesn't pick up live-app/ as a test root.
export default defineConfig({
  root: "live-app",
  plugins: [viteSingleFile()],
  build: {
    outDir: "../dist-live",
    emptyOutDir: true,
  },
});
