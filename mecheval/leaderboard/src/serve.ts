// Tiny static-file server for `mecheval/leaderboard/dist/`. Saves a
// dev dependency and matches the rest of the project's no-bundler vibe.

import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PORT = parseInt(process.env.PORT ?? "5174", 10);
// Anchor to the compiled script (mecheval/leaderboard/dist/serve.js) so the
// server works regardless of cwd — matters because turbo runs `dev` from the
// workspace package directory, not the repo root.
const ROOT = dirname(fileURLToPath(import.meta.url));

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
};

const server = createServer(async (req, res) => {
  const urlPath = (req.url ?? "/").split("?")[0];
  let p = urlPath === "/" ? "/index.html" : urlPath;
  // Defense against escaping the root.
  p = p.replace(/\.\./g, "");
  const fsPath = join(ROOT, p);
  try {
    const st = await stat(fsPath);
    if (st.isDirectory()) {
      res.writeHead(302, { Location: urlPath.replace(/\/?$/, "/") + "index.html" });
      res.end();
      return;
    }
    const ext = extname(fsPath);
    const buf = await readFile(fsPath);
    res.writeHead(200, {
      "Content-Type": MIME[ext] ?? "application/octet-stream",
      "Cache-Control": "no-cache",
    });
    res.end(buf);
  } catch {
    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end(`not found: ${p}`);
  }
});

server.listen(PORT, () => {
  console.log(`mecheval leaderboard at http://localhost:${PORT}/`);
});
