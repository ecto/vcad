#!/usr/bin/env node
// Build the self-contained npm tarball staging dir for @vcad/mcp.
//
// Mirrors services/mcp/build.sh (the Vercel bundle) for the npx/stdio
// distribution channel: one esbuild bundle + the kernel .wasm co-located
// beside it, version/sha/time baked in via --define so server_info can
// fingerprint the exact build. Output lands in packages/mcp/npm-dist/ with
// a generated minimal package.json (no workspace deps — they're bundled),
// ready for `npm publish ./npm-dist`.
//
// Prereq: workspace dists are built (npm run build --workspaces), since
// esbuild resolves @vcad/* through their package exports to dist/.
//
// Env:
//   VCAD_NPM_VERSION  overrides the published version (CI appends -main.N)

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const mcpDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const repo = join(mcpDir, "..", "..");
const out = join(mcpDir, "npm-dist");
const run = (cmd, args, cwd = mcpDir) =>
  execFileSync(cmd, args, { cwd, stdio: "inherit" });

const pkg = require(join(mcpDir, "package.json"));
const version = process.env.VCAD_NPM_VERSION || pkg.version;
let sha = "unknown";
try {
  sha = execFileSync("git", ["-C", repo, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim();
} catch {
  /* shallow/exported tree */
}
const buildTime = new Date().toISOString();

// Generated .ts assets (viewer html, live html, mech catalog) must exist
// before bundling from src.
run("node", [join(mcpDir, "scripts", "wrap-viewer.mjs")]);
run("node", [join(mcpDir, "scripts", "wrap-live.mjs")]);
run("node", [join(mcpDir, "scripts", "gen-mech-catalog.mjs")]);

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

console.log(`[build-npm] bundling @vcad/mcp@${version} (${sha.slice(0, 7)})`);
run("npx", [
  "esbuild@latest",
  join(mcpDir, "src", "npx-entry.ts"),
  "--bundle",
  "--platform=node",
  "--target=node20",
  "--format=esm",
  // Native rasterizer ships as an optionalDependency instead of being bundled
  // (.node binaries); render.ts degrades to raw SVG when the import fails.
  "--external:@resvg/resvg-js",
  `--define:__VCAD_VERSION__="${version}"`,
  `--define:__VCAD_BUILD_SHA__="${sha}"`,
  `--define:__VCAD_BUILD_TIME__="${buildTime}"`,
  `--outfile=${join(out, "index.mjs")}`,
  // esbuild hoists npx-entry.ts's own shebang to line 1; the banner adds the
  // aliased createRequire shim services/mcp/build.sh documents (bundled
  // sources emit their own top-level createRequire import, so the alias
  // avoids a redeclare).
  "--banner:js=import { createRequire as __vcadCreateRequire } from 'module'; const require = __vcadCreateRequire(import.meta.url);",
]);

copyFileSync(
  join(repo, "packages", "kernel-wasm", "vcad_kernel_wasm_bg.wasm"),
  join(out, "vcad_kernel_wasm_bg.wasm"),
);

writeFileSync(
  join(out, "package.json"),
  JSON.stringify(
    {
      name: "@vcad/mcp",
      version,
      description:
        "vcad MCP server — parametric CAD + PCB design tools for AI agents (self-contained: bundled server + kernel WASM)",
      license: "MIT",
      type: "module",
      bin: { "vcad-mcp": "index.mjs" },
      engines: { node: ">=20" },
      // Best-effort native PNG rasterizer; server falls back to SVG without it.
      optionalDependencies: { "@resvg/resvg-js": "^2" },
      repository: { type: "git", url: "https://github.com/ecto/vcad" },
      homepage: "https://vcad.io",
      vcadBuild: { sha, builtAt: buildTime },
    },
    null,
    2,
  ) + "\n",
);

writeFileSync(
  join(out, "README.md"),
  `# @vcad/mcp

vcad's MCP server as a self-contained bundle (server + BRep kernel WASM).

\`\`\`json
{ "mcpServers": { "vcad": { "command": "npx", "args": ["-y", "@vcad/mcp"] } } }
\`\`\`

Built from ${sha} at ${buildTime}. Source: https://github.com/ecto/vcad
`,
);

console.log(`[build-npm] staged ${out} — publish with: npm publish ./npm-dist --access public`);
