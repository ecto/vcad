// MCP-driven Vercel AI Gateway solver: the claude-mcp agentic loop in
// OpenAI chat-completions tool-calling format, so ANY gateway model —
// GPT, Gemini, Grok, Llama, DeepSeek, … — drives @vcad/mcp with the
// same tool surface. One key (AI_GATEWAY_API_KEY), every provider,
// full agentic treatment.
//
// Solver IDs follow `gateway-mcp-<provider>-<model>`; the leaderboard
// canonicalizes away the harness tokens, so gateway-mcp and
// gateway-direct runs of the same model aggregate as one identity.

import { performance } from "node:perf_hooks";
import type { Solver, SolverOutput, ToolCall } from "../solver.js";
import {
  BENCHMARK_EXCLUDED_TOOLS,
  PCB_SYSTEM_PROMPT,
  SYSTEM_PROMPT,
  connectMcp,
  defaultMcpBin,
  extractDocumentId,
  extractVcadJson,
  findDocumentId,
  mcpResultToText,
} from "./claude-mcp.js";
import { gatewayIdSegment } from "./gateway-direct.js";

const GATEWAY_BASE_URL = "https://ai-gateway.vercel.sh/v1";

export interface GatewayMcpConfig {
  /** Gateway model slug, "<provider>/<model>". */
  model: string;
  maxOutputTokens: number;
  maxIterations: number;
  baseUrl: string;
  mcpBin?: string;
}

export const DEFAULT_GATEWAY_MCP: GatewayMcpConfig = {
  model: "anthropic/claude-sonnet-4.5",
  maxOutputTokens: 8000,
  maxIterations: 30,
  baseUrl: GATEWAY_BASE_URL,
};

interface OpenAiToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

type CacheControl = { type: "ephemeral" };
type ContentPart = { type: "text"; text: string; cache_control?: CacheControl };

type OpenAiMessage =
  | { role: "system"; content: ContentPart[] }
  | { role: "user"; content: string }
  | { role: "assistant"; content: string | null; tool_calls?: OpenAiToolCall[] }
  | { role: "tool"; tool_call_id: string; content: ContentPart[] };

/** Sliding prompt-cache breakpoints, verified against the gateway's
 *  OpenAI-compat endpoint: anthropic models honor `cache_control` on
 *  content parts (12x cheaper on the cached prefix, measured); other
 *  providers ignore the field harmlessly (openai auto-caches prefixes
 *  on its own). Strategy mirrors claude-mcp: one breakpoint on the
 *  system block (which also covers the tool schemas — they precede it
 *  in anthropic's prompt order) plus the two most recent tool results,
 *  so the reusable prefix grows with the conversation. */
export function applyGatewayCacheBreakpoints(messages: OpenAiMessage[]): void {
  let marked = 0;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role !== "tool" && m.role !== "system") continue;
    for (const part of m.content) delete part.cache_control;
    if (m.role === "tool" && marked < 2 && m.content.length > 0) {
      m.content[m.content.length - 1].cache_control = { type: "ephemeral" };
      marked++;
    }
  }
}

/** Build a Solver bound to the given config. */
export function makeGatewayMcpSolver(cfg: Partial<GatewayMcpConfig> = {}): Solver {
  const config: GatewayMcpConfig = { ...DEFAULT_GATEWAY_MCP, ...cfg };
  const provider = config.model.includes("/")
    ? config.model.split("/", 1)[0]
    : "gateway";
  return {
    id: `gateway-mcp-${gatewayIdSegment(config.model)}`,
    name: `AI Gateway (MCP, ${config.model})`,
    provider,
    params: {
      mode: "mcp",
      model: config.model,
      max_tokens: config.maxOutputTokens,
      max_iterations: config.maxIterations,
    },
    async solve(task, prompt): Promise<SolverOutput> {
      const start = performance.now();
      const apiKey = process.env.AI_GATEWAY_API_KEY;
      if (!apiKey) {
        throw new Error(
          "gateway-mcp solver requires AI_GATEWAY_API_KEY in the environment.",
        );
      }
      const mcpBin = config.mcpBin ?? defaultMcpBin();
      const mcp = await connectMcp(mcpBin);
      const trace: ToolCall[] = [];
      let nextN = 0;
      let totalIn = 0;
      let totalOut = 0;
      let totalCacheCreate = 0;
      let totalCacheRead = 0;
      let lastDocumentId: string | null = null;

      const recordToolRaw = async (
        name: string,
        args: Record<string, unknown>,
      ) => {
        const t0 = performance.now();
        try {
          const result = await mcp.callTool({ name, arguments: args });
          const ms = performance.now() - t0;
          trace.push({
            n: nextN++,
            tool: name,
            args,
            result_kind: result.isError ? "err" : "ok",
            wallclock_ms: ms,
          });
          if (!result.isError) {
            const did = findDocumentId(result);
            if (did) lastDocumentId = did;
          }
          return { result };
        } catch (e) {
          const ms = performance.now() - t0;
          trace.push({
            n: nextN++,
            tool: name,
            args,
            result_kind: "err",
            wallclock_ms: ms,
          });
          return { error: (e as Error).message };
        }
      };

      try {
        // 1. Open a document for the agent to edit.
        const openRaw = await recordToolRaw("open_document", {});
        if ("error" in openRaw) {
          throw new Error(`open_document failed: ${openRaw.error}`);
        }
        lastDocumentId = extractDocumentId(openRaw.result);

        // 2. Advertise the MCP tool surface in OpenAI function format.
        const toolList = await mcp.listTools();
        const openAiTools = toolList.tools
          .filter((t) => !BENCHMARK_EXCLUDED_TOOLS.has(t.name))
          .map((t) => ({
            type: "function" as const,
            function: {
              name: t.name,
              description: t.description ?? "",
              parameters: t.inputSchema as Record<string, unknown>,
            },
          }));

        const isPcbTask = task.suite === "E" || task.suite === "P";
        const messages: OpenAiMessage[] = [
          {
            role: "system",
            content: [
              {
                type: "text",
                text: isPcbTask ? PCB_SYSTEM_PROMPT : SYSTEM_PROMPT,
                cache_control: { type: "ephemeral" },
              },
            ],
          },
          {
            role: "user",
            content: `${prompt}\n\nThe document_id for this session is "${lastDocumentId}". Use it on every tool call that requires it (unless a tool returns a new document_id — then switch to that one). Stop calling tools once you believe the document satisfies the task.`,
          },
        ];

        const { default: OpenAI } = await import("openai");
        const client = new OpenAI({ apiKey, baseURL: config.baseUrl });

        // 3. Agentic loop.
        for (let iter = 0; iter < config.maxIterations; iter++) {
          applyGatewayCacheBreakpoints(messages);
          const resp = await client.chat.completions.create({
            model: config.model,
            messages: messages as never,
            tools: openAiTools as never,
            max_tokens: config.maxOutputTokens,
          });
          // Gateway usage: prompt_tokens INCLUDES cached + cache-write
          // tokens; the details break them out for cost analysis.
          const u = resp.usage as
            | (typeof resp.usage & {
                prompt_tokens_details?: { cached_tokens?: number };
                cache_creation_input_tokens?: number;
              })
            | undefined;
          totalIn += u?.prompt_tokens ?? 0;
          totalOut += u?.completion_tokens ?? 0;
          totalCacheRead += u?.prompt_tokens_details?.cached_tokens ?? 0;
          totalCacheCreate += u?.cache_creation_input_tokens ?? 0;

          const msg = resp.choices[0]?.message;
          if (!msg) break;
          messages.push({
            role: "assistant",
            content: msg.content ?? null,
            tool_calls: (msg.tool_calls ?? []) as OpenAiToolCall[],
          });

          const calls = (msg.tool_calls ?? []) as OpenAiToolCall[];
          if (calls.length === 0) break;

          for (const call of calls) {
            const name = call.function.name;
            let args: Record<string, unknown> = {};
            try {
              args = JSON.parse(call.function.arguments || "{}");
            } catch {
              messages.push({
                role: "tool",
                tool_call_id: call.id,
                content: [{ type: "text", text: "ERROR: tool arguments were not valid JSON." }],
              });
              trace.push({
                n: nextN++,
                tool: name,
                args: call.function.arguments,
                result_kind: "err",
                wallclock_ms: 0,
              });
              continue;
            }
            if (BENCHMARK_EXCLUDED_TOOLS.has(name)) {
              trace.push({
                n: nextN++,
                tool: name,
                args,
                result_kind: "err",
                wallclock_ms: 0,
              });
              messages.push({
                role: "tool",
                tool_call_id: call.id,
                content: [
                  { type: "text", text: `ERROR: ${name} is not available during benchmark runs.` },
                ],
              });
              continue;
            }
            const out = await recordToolRaw(name, args);
            messages.push({
              role: "tool",
              tool_call_id: call.id,
              content: [
                {
                  type: "text",
                  text: "error" in out ? `ERROR: ${out.error}` : mcpResultToText(out.result),
                },
              ],
            });
          }
        }

        // 4. Read the final document from the latest session.
        const docRaw = await recordToolRaw("get_document", {
          document_id: lastDocumentId,
        });
        if ("error" in docRaw) {
          throw new Error(`get_document failed: ${docRaw.error}`);
        }
        if (docRaw.result.isError) {
          throw new Error(`get_document failed: ${mcpResultToText(docRaw.result)}`);
        }
        const vcadJson = extractVcadJson(docRaw.result);

        const wallclockSec = (performance.now() - start) / 1000;
        return {
          vcadJson,
          controlPolicy: null,
          toolCalls: trace,
          tokens: {
            input: totalIn,
            output: totalOut,
            total: totalIn + totalOut,
            cache_creation_input: totalCacheCreate,
            cache_read_input: totalCacheRead,
          },
          wallclockSec,
        };
      } finally {
        try {
          await mcp.close();
        } catch {
          // best-effort
        }
      }
    },
  };
}

export const gatewayMcpSolver = makeGatewayMcpSolver();
