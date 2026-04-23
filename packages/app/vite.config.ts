/// <reference types="vitest" />
import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import basicSsl from "@vitejs/plugin-basic-ssl";
import tailwindcss from "@tailwindcss/vite";
import wasm from "vite-plugin-wasm";
import topLevelAwait from "vite-plugin-top-level-await";
import { VitePWA } from "vite-plugin-pwa";
import { fileURLToPath } from "url";
import { dirname, resolve } from "path";
import { createClient } from "@supabase/supabase-js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

/**
 * esbuild plugin: Disables R3F's reconciler profiling marks in dev mode.
 *
 * React 19's dev reconciler runs expensive profiling (prop serialization + performance.measure)
 * for every fiber on every render, costing 10-15s on R3F scenes. The profiling is gated by a
 * `Me` flag that checks for `console.timeStamp` and `performance.measure` support — always true
 * in modern browsers. This patches the flag to `false`, skipping profiling while keeping all
 * other dev reconciler functionality (warnings, getOwner, etc.) intact.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function r3fDisableProfilingPlugin(): any {
  return {
    name: "r3f-disable-profiling",
    setup(build: any) {
      build.onLoad(
        { filter: /events-.*\.esm\.js$/ },
        async (args: any) => {
          if (!args.path.includes("@react-three/fiber")) return;
          const fs = await import("fs");
          let contents = await fs.promises.readFile(args.path, "utf8");
          // Disable the profiling flag: Me = typeof console < "u" && ... → Me = false
          const pattern = /(\w+)\s*=\s*typeof console\s*<\s*"u"\s*&&\s*typeof console\.timeStamp\s*==\s*"function"\s*&&\s*typeof performance\s*<\s*"u"\s*&&\s*typeof performance\.measure\s*==\s*"function"/;
          if (pattern.test(contents)) {
            contents = contents.replace(pattern, "$1 = false");
            return { contents, loader: "js" };
          }
        },
      );
    },
  };
}

/**
 * Injects a `<link rel="preload">` for the kernel WASM into index.html at
 * build time. The browser then starts the ~multi-MB fetch as soon as the
 * HTML streams in — well before main.tsx parses — overlapping it with
 * JS parse and React's first render. Build-only: in dev, the wasm is
 * served from an outside-root path Vite rewrites at runtime, so the
 * savings aren't worth the extra moving part.
 */
function preloadKernelWasmPlugin(): Plugin {
  return {
    name: "preload-kernel-wasm",
    apply: "build",
    transformIndexHtml: {
      order: "post",
      handler(html, ctx) {
        if (!ctx.bundle) return html;
        const wasmAsset = Object.keys(ctx.bundle).find((n) =>
          /vcad_kernel_wasm.*\.wasm$/.test(n),
        );
        if (!wasmAsset) return html;
        const link = `<link rel="preload" href="/${wasmAsset}" as="fetch" type="application/wasm" crossorigin>`;
        return html.replace("</head>", `    ${link}\n  </head>`);
      },
    },
  };
}

/** Dev-only plugin that handles /api/generate requests */
function devApiPlugin(env: Record<string, string>): Plugin {
  const SYSTEM_PROMPT =
    "You are a CAD assistant. Output only VCode code (C for box, Y for cylinder, T for translate, U for union, D for difference). No explanations, just the IR code.";

  function formatChatPrompt(userPrompt: string): string {
    return `<|im_start|>system\n${SYSTEM_PROMPT}<|im_end|>\n<|im_start|>user\n${userPrompt}<|im_end|>\n<|im_start|>assistant\n`;
  }

  function cleanGeneratedIR(text: string): string {
    let ir = text.trim();
    if (ir.startsWith("```")) {
      ir = ir.replace(/^```(?:ir|text|plaintext)?\n?/, "").replace(/\n?```$/, "");
    }
    const stopPatterns = ["\n\n", "User", "user", "Now:", "Assistant", "Design:", "<|im_end|>", "<|im_start|>"];
    for (const pattern of stopPatterns) {
      const idx = ir.indexOf(pattern);
      if (idx > 0) ir = ir.substring(0, idx);
    }
    const lines = ir.split("\n");
    const validOpcodes = ["C", "Y", "S", "K", "T", "R", "X", "U", "D", "I", "LP", "CP", "SH", "FI", "CH", "SK", "L", "A", "E", "V", "M", "ROOT", "PDEF", "INST", "END"];
    const minArgs: Record<string, number> = { C: 3, Y: 2, S: 1, K: 3, T: 4, R: 4, X: 4, U: 2, D: 2, I: 2, SH: 2, FI: 2, CH: 2, LP: 4, CP: 4, SK: 1, L: 4, A: 7, E: 2, V: 2, M: 4, ROOT: 1, PDEF: 1, INST: 2, END: 0 };
    const validLines: string[] = [];
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith("#")) continue;
      const parts = trimmed.split(/\s+/);
      const opcode = parts[0] ?? "";
      if (!validOpcodes.includes(opcode)) break;
      const required = minArgs[opcode] ?? 0;
      if (parts.length < required + 1) break;
      validLines.push(line);
    }
    return validLines.join("\n").trim();
  }

  // Route → handler file mapping for dev-time delegation to Vercel functions.
  // Each production api/*.ts file is loaded on demand when the matching URL
  // is hit. Keeping this as a single table means adding a new endpoint only
  // requires editing this list (plus the file itself).
  const ENDPOINT_ROUTES: Array<{
    path: string;
    file: string;
    methods: readonly string[];
  }> = [
    { path: "/api/chat", file: "api/chat.ts", methods: ["POST", "OPTIONS"] },
    { path: "/api/usage", file: "api/usage.ts", methods: ["GET", "OPTIONS"] },
    { path: "/api/billing/checkout", file: "api/billing/checkout.ts", methods: ["POST", "OPTIONS"] },
    { path: "/api/billing/portal", file: "api/billing/portal.ts", methods: ["POST", "OPTIONS"] },
    { path: "/api/billing/webhook", file: "api/billing/webhook.ts", methods: ["POST"] },
  ];

  // Env var names the dev delegator mirrors from Vite's env into process.env
  // so server handlers can read them as if running on Vercel.
  const ENV_PASSTHROUGH = [
    "ANTHROPIC_API_KEY",
    "SUPABASE_URL",
    "SUPABASE_SERVICE_ROLE_KEY",
    "IP_HASH_SALT",
    "STRIPE_SECRET_KEY",
    "STRIPE_WEBHOOK_SECRET",
    "APP_ORIGIN",
  ] as const;

  return {
    name: "dev-api",
    configureServer(server) {
      // Generic delegator: matches each incoming URL against ENDPOINT_ROUTES,
      // lazy-imports the production handler, and dispatches via the same
      // VercelRequest/Response shim. A single middleware handles every route
      // in the table so adding /api/something is one line instead of a new
      // 60-line middleware block.
      server.middlewares.use(async (req, res, next) => {
        const url = req.url ?? "";
        // Strip query string so "/api/usage?ts=..." still matches.
        const pathname = url.split("?")[0] ?? url;
        const match = ENDPOINT_ROUTES.find((r) => r.path === pathname);
        if (!match) return next();
        if (!match.methods.includes(req.method ?? "")) return next();

        for (const key of ENV_PASSTHROUGH) {
          if (env[key] && !process.env[key]) {
            process.env[key] = env[key];
          }
        }

        try {
          const mod = await server.ssrLoadModule(
            resolve(__dirname, match.file),
          );

          // The production handler is written for Vercel's VercelRequest/Response
          // which adds helpers like res.status().json(), req.body auto-parsing,
          // etc. Node's raw http.ServerResponse doesn't have those, so we shim
          // a minimal subset onto the Node objects in-place before dispatching.
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          const resShim = res as any;
          if (typeof resShim.status !== "function") {
            resShim.status = (code: number) => {
              resShim.statusCode = code;
              return resShim;
            };
          }
          if (typeof resShim.json !== "function") {
            resShim.json = (payload: unknown) => {
              resShim.setHeader("Content-Type", "application/json");
              resShim.end(JSON.stringify(payload));
              return resShim;
            };
          }

          // Cast: node's http req/res are structurally compatible with
          // the bits api/*.ts actually use (headers, statusCode, end).
          await mod.default(req as never, res as never);
        } catch (err) {
          console.error(`[dev-api] ${match.path} delegation error:`, err);
          if (!res.headersSent) {
            res.statusCode = 500;
            res.end(JSON.stringify({ error: `dev ${match.path} delegation failed` }));
          } else {
            try { res.end(); } catch { /* noop */ }
          }
        }
      });

      // ── /api/generate — cad0 inference endpoint ──────────────
      server.middlewares.use(async (req, res, next) => {
        if (req.url !== "/api/generate" || req.method !== "POST") {
          return next();
        }

        // CORS
        res.setHeader("Access-Control-Allow-Origin", "*");
        res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
        res.setHeader("Access-Control-Allow-Headers", "Authorization, Content-Type");

        // Parse body
        let body = "";
        for await (const chunk of req) body += chunk;
        const { prompt } = JSON.parse(body);

        if (!prompt) {
          res.statusCode = 400;
          res.end(JSON.stringify({ error: "Prompt required" }));
          return;
        }

        // Verify auth
        const authHeader = req.headers.authorization;
        if (!authHeader?.startsWith("Bearer ")) {
          res.statusCode = 401;
          res.end(JSON.stringify({ error: "Unauthorized" }));
          return;
        }

        const supabase = createClient(env.SUPABASE_URL, env.SUPABASE_ANON_KEY);
        const { data: { user }, error: authError } = await supabase.auth.getUser(authHeader.slice(7));
        if (authError || !user) {
          res.statusCode = 401;
          res.end(JSON.stringify({ error: "Unauthorized" }));
          return;
        }

        // Call HF
        const startTime = Date.now();
        try {
          const hfResponse = await fetch(env.HF_INFERENCE_ENDPOINT, {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              Authorization: `Bearer ${env.HF_TOKEN}`,
            },
            body: JSON.stringify({
              inputs: formatChatPrompt(prompt),
              parameters: { max_new_tokens: 512, temperature: 0.1, do_sample: true, return_full_text: false },
            }),
          });

          if (!hfResponse.ok) {
            throw new Error(`HF inference failed: ${await hfResponse.text()}`);
          }

          const result = await hfResponse.json() as { generated_text?: string }[] | { generated_text?: string; text?: string };
          const generatedText = Array.isArray(result) ? result[0]?.generated_text ?? "" : result.generated_text ?? result.text ?? "";
          const ir = cleanGeneratedIR(generatedText);

          // Log to DB (fire and forget)
          const supabaseAdmin = createClient(env.SUPABASE_URL, env.SUPABASE_SERVICE_ROLE_KEY);
          supabaseAdmin.from("inference_logs").insert({
            user_id: user.id,
            prompt,
            result: ir,
            tokens: generatedText.length,
            duration_ms: Date.now() - startTime,
          }).then(() => {});

          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ ir, tokens: generatedText.length, durationMs: Date.now() - startTime }));
        } catch (err) {
          const errorMsg = err instanceof Error ? err.message : "AI inference failed";
          res.statusCode = 500;
          res.end(JSON.stringify({ error: errorMsg }));
        }
      });
    },
  };
}

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, resolve(__dirname, "../.."), "");

  return {
    server: {
      allowedHosts: ["mew"],
    },
    envDir: "../../",
    define: {
      __APP_VERSION__: JSON.stringify(process.env.npm_package_version || "0.0.0"),
      __BUILD_TIME__: JSON.stringify(new Date().toISOString()),
    },
    plugins: [
      !process.env.TAURI_DEV && basicSsl(),
      devApiPlugin(env),
      preloadKernelWasmPlugin(),
      react(),
      tailwindcss(),
      wasm(),
      topLevelAwait(),
      VitePWA({
        registerType: "prompt",
        devOptions: {
          enabled: false,
        },
        includeAssets: ["fonts/**/*", "assets/**/*"],
        manifest: false,
        workbox: {
          globPatterns: ["**/*.{js,css,html,woff,woff2,otf,wasm}"],
          globIgnores: ["**/ort-*.wasm"],
          maximumFileSizeToCacheInBytes: 12 * 1024 * 1024,
          runtimeCaching: [
            {
              urlPattern: /\.(woff|woff2|otf|ttf)$/,
              handler: "CacheFirst",
              options: { cacheName: "font-cache", expiration: { maxEntries: 20, maxAgeSeconds: 60 * 60 * 24 * 365 } },
            },
            {
              urlPattern: /\.(png|jpg|jpeg|svg|gif|webp)$/,
              handler: "CacheFirst",
              options: { cacheName: "image-cache", expiration: { maxEntries: 50, maxAgeSeconds: 60 * 60 * 24 * 30 } },
            },
          ],
        },
      }),
    ],
    resolve: {
      alias: {
        "@": resolve(__dirname, "./src"),
        // Force a single resolution of kernel-wasm to prevent double-instantiation.
        // Without this, `@vcad/kernel-wasm` and `vcad-kernel-wasm` (package's internal name)
        // can resolve to different module instances, each with its own WASM memory,
        // causing "Out of bounds memory access" when pointers cross instances.
        "@vcad/kernel-wasm": resolve(__dirname, "../kernel-wasm/vcad_kernel_wasm.js"),
        "vcad-kernel-wasm": resolve(__dirname, "../kernel-wasm/vcad_kernel_wasm.js"),
      },
      // Force a single React instance across the workspace. Without this,
      // workspace packages (@vcad/auth, @vcad/core, etc.) can resolve their own
      // React copies, which throws "null is not an object (evaluating
      // 'resolveDispatcher().useState')" when components from different
      // packages mount together. react/jsx-runtime is dedup'd separately
      // because vite's optimizer treats it as a distinct import.
      dedupe: [
        "@vcad/kernel-wasm",
        "vcad-kernel-wasm",
        "react",
        "react-dom",
        "react/jsx-runtime",
        "react/jsx-dev-runtime",
      ],
    },
    worker: {
      format: "es",
      plugins: () => [wasm(), topLevelAwait()],
    },
    build: {
      rollupOptions: {
        output: {
          manualChunks: {
            three: ["three"],
            "three-fiber": ["@react-three/fiber"],
            "three-drei": ["@react-three/drei"],
            "three-postprocessing": ["@react-three/postprocessing"],
          },
        },
      },
    },
    optimizeDeps: {
      exclude: ["@vcad/kernel-wasm", "@vcad/engine"],
      // Crawl the entire src/ tree (not just the html entry + statically
      // imported modules) so deps that only appear inside lazy/dynamic chunks
      // — e.g. the phosphor-icons used inside FeatureTree — get into the
      // initial prebundle. Without this, vite re-runs optimizeDeps the first
      // time a lazy chunk imports a previously-unseen icon, invalidating the
      // prebundle hash and 504'ing every chunk that's already in flight.
      entries: ["./index.html", "./src/**/*.{ts,tsx}"],
      // Force-include the AI Elements / streamdown stack in the prebundle so
      // they all use the deduped React instance. Without this, vite can lazily
      // optimize them on first import with a stale React snapshot, triggering
      // the dispatcher null error.
      include: [
        "react",
        "react-dom",
        "react-dom/client",
        "react/jsx-runtime",
        "streamdown",
        "use-stick-to-bottom",
        "lucide-react",
      ],
      esbuildOptions: {
        plugins: [r3fDisableProfilingPlugin()],
      },
    },
    test: {
      environment: "happy-dom",
      setupFiles: ["./src/test/setup.ts"],
      include: ["src/**/*.test.{ts,tsx}"],
    },
  };
});
