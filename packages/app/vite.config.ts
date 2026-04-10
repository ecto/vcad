/// <reference types="vitest" />
import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
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

  return {
    name: "dev-api",
    configureServer(server) {
      // ── /api/chat — AI chat endpoint (dev only) ──────────────
      server.middlewares.use(async (req, res, next) => {
        if (req.url !== "/api/chat" || (req.method !== "POST" && req.method !== "OPTIONS")) {
          return next();
        }

        res.setHeader("Access-Control-Allow-Origin", "*");
        res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
        res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization");

        if (req.method === "OPTIONS") {
          res.statusCode = 200;
          res.end();
          return;
        }

        let body = "";
        for await (const chunk of req) body += chunk;
        const { messages, context } = JSON.parse(body);

        if (!messages?.length) {
          res.statusCode = 400;
          res.end(JSON.stringify({ error: "messages required" }));
          return;
        }

        const apiKey = env.ANTHROPIC_API_KEY;
        if (!apiKey) {
          res.statusCode = 503;
          res.end(JSON.stringify({ error: "Set ANTHROPIC_API_KEY in .env.local for local chat development" }));
          return;
        }

        // Build system prompt with context
        let systemPrompt = `You are vcad's AI assistant — a parametric CAD copilot. Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters. Be concise.

When asked to create or modify geometry, use the available tools. After a tool call, briefly confirm what you did.
When the user refers to "this" or "it" without specifics, use the selected geometry context provided.`;
        if (context?.selectedParts?.length) {
          const partList = context.selectedParts
            .map((p: { partName: string; geometryType: string; partId: string }) => `- ${p.partName} (${p.geometryType}, id: ${p.partId})`)
            .join("\n");
          systemPrompt += `\n\nCurrently selected geometry:\n${partList}`;
        }

        // Tool definitions for CAD operations
        const tools = [
          {
            name: "add_primitive",
            description: "Add a primitive shape to the scene. Returns the new part ID.",
            input_schema: {
              type: "object" as const,
              properties: {
                kind: { type: "string", enum: ["cube", "cylinder", "sphere"], description: "Primitive type" },
              },
              required: ["kind"],
            },
          },
          {
            name: "transform_part",
            description: "Translate, rotate, or scale a part. Coordinates are in mm, angles in degrees.",
            input_schema: {
              type: "object" as const,
              properties: {
                partId: { type: "string", description: "Part ID to transform. Use the ID from selected geometry context." },
                translate: { type: "object", properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } }, description: "Translation offset in mm" },
                rotate: { type: "object", properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } }, description: "Rotation angles in degrees" },
                scale: { type: "object", properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } }, description: "Scale factors" },
              },
              required: ["partId"],
            },
          },
          {
            name: "add_fillet",
            description: "Apply a fillet (rounded edge) to a part.",
            input_schema: {
              type: "object" as const,
              properties: {
                partId: { type: "string", description: "Part ID to fillet" },
                radius: { type: "number", description: "Fillet radius in mm" },
              },
              required: ["partId", "radius"],
            },
          },
          {
            name: "add_chamfer",
            description: "Apply a chamfer (beveled edge) to a part.",
            input_schema: {
              type: "object" as const,
              properties: {
                partId: { type: "string", description: "Part ID to chamfer" },
                distance: { type: "number", description: "Chamfer distance in mm" },
              },
              required: ["partId", "distance"],
            },
          },
          {
            name: "add_shell",
            description: "Hollow out a part, leaving walls of the specified thickness.",
            input_schema: {
              type: "object" as const,
              properties: {
                partId: { type: "string", description: "Part ID to shell" },
                thickness: { type: "number", description: "Wall thickness in mm" },
              },
              required: ["partId", "thickness"],
            },
          },
          {
            name: "apply_boolean",
            description: "Apply a boolean operation between two selected parts. Requires exactly 2 parts selected.",
            input_schema: {
              type: "object" as const,
              properties: {
                operation: { type: "string", enum: ["union", "difference", "intersection"], description: "Boolean operation type" },
              },
              required: ["operation"],
            },
          },
          {
            name: "delete_part",
            description: "Delete a part from the scene.",
            input_schema: {
              type: "object" as const,
              properties: {
                partId: { type: "string", description: "Part ID to delete" },
              },
              required: ["partId"],
            },
          },
          {
            name: "inspect_part",
            description: "Get information about a part: dimensions, volume, type.",
            input_schema: {
              type: "object" as const,
              properties: {
                partId: { type: "string", description: "Part ID to inspect" },
              },
              required: ["partId"],
            },
          },
          {
            name: "list_parts",
            description: "List all parts in the current document with their IDs, names, and types.",
            input_schema: {
              type: "object" as const,
              properties: {},
            },
          },
        ];

        try {
          const anthropicRes = await fetch("https://api.anthropic.com/v1/messages", {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              "x-api-key": apiKey,
              "anthropic-version": "2023-06-01",
            },
            body: JSON.stringify({
              model: "claude-sonnet-4-20250514",
              max_tokens: 1024,
              system: systemPrompt,
              stream: true,
              tools,
              // Pass content through as-is: can be string or array of content blocks
              // (text, tool_use, tool_result) for Anthropic's tool calling protocol
              messages,
            }),
          });

          if (!anthropicRes.ok) {
            const errText = await anthropicRes.text();
            res.statusCode = anthropicRes.status;
            res.end(errText);
            return;
          }

          // Stream as newline-delimited JSON events so client can distinguish text vs tool_use
          res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
          res.setHeader("Transfer-Encoding", "chunked");
          res.setHeader("Cache-Control", "no-cache");

          const reader = anthropicRes.body?.getReader();
          if (!reader) { res.end(); return; }

          const decoder = new TextDecoder();
          let sseBuffer = "";
          while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            sseBuffer += decoder.decode(value, { stream: true });
            const lines = sseBuffer.split("\n");
            sseBuffer = lines.pop() ?? "";
            for (const line of lines) {
              if (!line.startsWith("data: ")) continue;
              const data = line.slice(6);
              if (data === "[DONE]") continue;
              try {
                const event = JSON.parse(data);
                if (event.type === "content_block_delta" && event.delta?.type === "text_delta") {
                  res.write(`data: ${JSON.stringify({ type: "text", text: event.delta.text })}\n\n`);
                } else if (event.type === "content_block_start" && event.content_block?.type === "tool_use") {
                  res.write(`data: ${JSON.stringify({ type: "tool_start", id: event.content_block.id, name: event.content_block.name })}\n\n`);
                } else if (event.type === "content_block_delta" && event.delta?.type === "input_json_delta") {
                  res.write(`data: ${JSON.stringify({ type: "tool_delta", json: event.delta.partial_json })}\n\n`);
                } else if (event.type === "content_block_stop") {
                  res.write(`data: ${JSON.stringify({ type: "block_stop" })}\n\n`);
                } else if (event.type === "message_stop") {
                  res.write(`data: ${JSON.stringify({ type: "done" })}\n\n`);
                }
              } catch { /* skip non-JSON */ }
            }
          }
          res.end();
        } catch (err) {
          console.error("Chat API error:", err);
          res.statusCode = 500;
          res.end(JSON.stringify({ error: "Chat API error" }));
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
      devApiPlugin(env),
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
          maximumFileSizeToCacheInBytes: 10 * 1024 * 1024,
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
      dedupe: ["@vcad/kernel-wasm", "vcad-kernel-wasm"],
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
      exclude: ["@vcad/kernel-wasm"],
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
