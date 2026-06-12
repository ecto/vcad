// MCP-driven Claude solver: spawns @vcad/mcp as a stdio subprocess,
// forwards the model's tool_use blocks to MCP, returns tool results to
// the model, and extracts the final .vcad from the session document
// when the model stops issuing tool calls.
//
// This is the "real" agentic version. claude-direct is the single-shot
// floor; claude-mcp is what shows up on the leaderboard as the actual
// frontier-model number.

import { createRequire } from "node:module";
import { performance } from "node:perf_hooks";
import type Anthropic from "@anthropic-ai/sdk";
import type { Solver, SolverOutput, ToolCall } from "../solver.js";
import type { Task } from "../task.js";

const require = createRequire(import.meta.url);

export interface ClaudeMcpConfig {
  model: string;
  /** Per-API-call output cap. */
  maxOutputTokens: number;
  /** Hard cap on agent iterations (one round-trip with the model =
   *  one iteration). Acts as a runaway-loop circuit breaker. */
  maxIterations: number;
  /** Path to the vcad-mcp binary. Defaults to the resolved
   *  @vcad/mcp main export, which the harness picks up via npm
   *  workspaces. Set MECHEVAL_VCAD_MCP_BIN to override. */
  mcpBin?: string;
  /** Test escape hatch: when present, return this string as the
   *  Anthropic reply instead of calling the API. The MCP loop is
   *  still skipped. Used by unit tests to validate plumbing. */
  fakeReplyForTests?: string;
}

export const DEFAULT_CLAUDE_MCP: ClaudeMcpConfig = {
  model: "claude-opus-4-7",
  maxOutputTokens: 8000,
  maxIterations: 30,
};

const SYSTEM_PROMPT = `You are a CAD modeling agent for vcad. You receive an engineering task and edit a vcad document via the MCP tool surface to satisfy it.

You are given a \`document_id\` for a fresh, empty document. Use the MCP tools to mutate it. The grader will read the final document from \`get_document\` after you stop.

Conventions:
- Z is up. Cube and cylinder primitives have their corner / base at the origin.
- Units are millimeters (mm).
- Output a single solid (one root) unless the task says otherwise.
- For a centered shape, translate the primitive into place — primitives are not auto-centered.
- For holes: subtract a cylinder from the body via a Difference op (the create_cad_loon tool can author IR DAGs in one shot when that's simpler).

When you believe the document satisfies the task, stop calling tools and reply with a one-sentence summary. Don't call \`close_document\` — the grader needs the document open to read it.`;

/** Self-grading oracle tools — letting the model grade itself against
 *  the task's own checks mid-run would contaminate the leaderboard. */
const ORACLE_TOOLS = new Set(["verify_part", "list_eval_tasks"]);

/** All tools excluded from scored benchmark runs: the oracle pair, plus
 *  close_document — agents occasionally call it mid-run despite the
 *  system prompt forbidding it, destroying the session before the final
 *  get_document (~3% of attempts in the 2026-06 matrix died this way).
 *  It serves no purpose in a scored run. Applied BOTH when advertising
 *  the tool list to the model and when executing tool_use blocks: the
 *  API normally constrains tool_use to declared tools, but a
 *  hallucinated call must not reach MCP either. render_view stays: eyes
 *  are product surface. */
export const BENCHMARK_EXCLUDED_TOOLS = new Set([
  ...ORACLE_TOOLS,
  "close_document",
]);

interface McpToolResult {
  content?: Array<{ type: string; text?: string; data?: string; mimeType?: string }>;
  isError?: boolean;
}

interface MinimalMcpClient {
  listTools(): Promise<{ tools: Array<{ name: string; description?: string; inputSchema: unknown }> }>;
  callTool(args: { name: string; arguments: Record<string, unknown> }): Promise<McpToolResult>;
  close(): Promise<void>;
}

async function connectMcp(mcpBin: string): Promise<MinimalMcpClient> {
  // Lazy imports — the SDK is heavy and only needed when this solver runs.
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { StdioClientTransport } = await import(
    "@modelcontextprotocol/sdk/client/stdio.js"
  );
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [mcpBin],
  });
  const client = new Client(
    { name: "mecheval-harness", version: "0.0.1" },
    { capabilities: {} },
  );
  await client.connect(transport);
  return client as unknown as MinimalMcpClient;
}

function defaultMcpBin(): string {
  if (process.env.MECHEVAL_VCAD_MCP_BIN) return process.env.MECHEVAL_VCAD_MCP_BIN;
  return require.resolve("@vcad/mcp");
}

/** Pull the first text content out of an MCP tool result. Errors and
 *  non-text content are surfaced as a stringified placeholder so the
 *  model still gets useful feedback. */
function mcpResultToText(result: McpToolResult): string {
  const parts = (result.content ?? []).map((c) =>
    c.type === "text" && c.text != null ? c.text : `[${c.type} content]`,
  );
  const body = parts.join("\n");
  return result.isError ? `ERROR: ${body}` : body;
}

type AnthropicToolResultBlock =
  | { type: "text"; text: string }
  | { type: "image"; source: { type: "base64"; media_type: string; data: string } };

/** Convert MCP content blocks to Anthropic tool_result blocks, forwarding
 *  image blocks intact so tools like render_view actually deliver pixels
 *  to the model — flattening them to a "[image content]" placeholder
 *  would make the agent's eyes cost tokens while showing nothing. */
function mcpResultToAnthropicContent(
  result: McpToolResult,
): AnthropicToolResultBlock[] {
  const blocks: AnthropicToolResultBlock[] = [];
  for (const c of result.content ?? []) {
    if (c.type === "text" && c.text != null) {
      blocks.push({ type: "text", text: c.text });
    } else if (c.type === "image" && c.data && c.mimeType?.startsWith("image/")) {
      blocks.push({
        type: "image",
        source: { type: "base64", media_type: c.mimeType, data: c.data },
      });
    } else {
      blocks.push({ type: "text", text: `[${c.type} content]` });
    }
  }
  if (blocks.length === 0) {
    blocks.push({ type: "text", text: "(empty result)" });
  }
  return blocks;
}

/** Find the session id in an open_document result. Parses each text
 *  block independently — the server may append preview-handle blocks
 *  for MCP Apps hosts, so the joined text is not valid JSON. */
export function extractDocumentId(openResult: McpToolResult): string {
  for (const c of openResult.content ?? []) {
    if (c.type !== "text" || c.text == null) continue;
    try {
      const parsed = JSON.parse(c.text);
      if (typeof parsed.document_id === "string") return parsed.document_id;
    } catch {
      // try next block
    }
  }
  throw new Error(
    `open_document did not return a JSON document_id; got: ${mcpResultToText(openResult)}`,
  );
}

/** Extract the Document IR JSON from a get_document result. Each text
 *  block is tried independently for the same reason as above — naively
 *  joining blocks corrupts the .vcad with trailing characters (this
 *  exact failure took down the first post-viewer matrix run). */
export function extractVcadJson(result: McpToolResult): string {
  for (const c of result.content ?? []) {
    if (c.type !== "text" || c.text == null) continue;
    try {
      const parsed = JSON.parse(c.text);
      if (
        parsed &&
        typeof parsed === "object" &&
        "nodes" in parsed &&
        "roots" in parsed
      ) {
        // Pretty-print so the run blob's persisted .vcad is readable.
        return JSON.stringify(parsed, null, 2);
      }
    } catch {
      // try next block
    }
  }
  throw new Error("get_document returned no parseable Document JSON block");
}

type CacheControl = { type: "ephemeral" };

interface AnthropicLite {
  messages: {
    create(req: {
      model: string;
      max_tokens: number;
      system:
        | string
        | Array<{ type: "text"; text: string; cache_control?: CacheControl }>;
      tools: Array<{
        name: string;
        description?: string;
        input_schema: unknown;
        cache_control?: CacheControl;
      }>;
      messages: Array<{ role: "user" | "assistant"; content: unknown }>;
    }): Promise<{
      stop_reason: string | null;
      content: Array<
        | { type: "text"; text: string }
        | { type: "tool_use"; id: string; name: string; input: Record<string, unknown> }
        | { type: string; [k: string]: unknown }
      >;
      usage: {
        input_tokens: number;
        output_tokens: number;
        cache_creation_input_tokens?: number | null;
        cache_read_input_tokens?: number | null;
      };
    }>;
  };
}

/** Mark the conversation for prompt caching: strip stale message-level
 *  breakpoints, then mark the last content block of the two most recent
 *  user messages. Two sliding breakpoints (plus one on tools and one on
 *  system = the 4-breakpoint API max) keep a reachable cache entry within
 *  the API's 20-block lookback even when a turn fans out into many
 *  parallel tool_use/tool_result blocks. Without caching every iteration
 *  re-reads the full history + ~45 tool schemas at full price (~178k
 *  input tokens for even a simple a1-tier task). */
export function applyConversationCacheBreakpoints(
  messages: Array<{ role: string; content: unknown }>,
): void {
  for (const message of messages) {
    if (!Array.isArray(message.content)) continue;
    for (const block of message.content as Array<Record<string, unknown>>) {
      if (block && typeof block === "object" && "cache_control" in block) {
        delete block.cache_control;
      }
    }
  }
  let marked = 0;
  for (let i = messages.length - 1; i >= 0 && marked < 2; i--) {
    const message = messages[i];
    if (message.role !== "user" || !Array.isArray(message.content)) continue;
    if (message.content.length === 0) continue;
    const last = message.content[message.content.length - 1] as Record<
      string,
      unknown
    >;
    if (!last || typeof last !== "object") continue;
    last.cache_control = { type: "ephemeral" } satisfies CacheControl;
    marked++;
  }
}

async function makeAnthropic(): Promise<AnthropicLite> {
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    throw new Error("claude-mcp solver requires ANTHROPIC_API_KEY in the environment.");
  }
  const { default: Anthropic } = await import("@anthropic-ai/sdk");
  return new Anthropic({ apiKey }) as unknown as AnthropicLite;
}

/** Build a Solver bound to the given config. */
export function makeClaudeMcpSolver(cfg: Partial<ClaudeMcpConfig> = {}): Solver {
  const config: ClaudeMcpConfig = { ...DEFAULT_CLAUDE_MCP, ...cfg };
  return {
    id: `claude-mcp-${config.model}`,
    name: `Claude (MCP, ${config.model})`,
    provider: "anthropic",
    params: {
      mode: "mcp",
      model: config.model,
      max_tokens: config.maxOutputTokens,
      max_iterations: config.maxIterations,
    },
    async solve(task, prompt): Promise<SolverOutput> {
      const start = performance.now();
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
      ): Promise<{ result: McpToolResult } | { error: string }> => {
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

      const recordTool = async (
        name: string,
        args: Record<string, unknown>,
      ): Promise<string> => {
        const out = await recordToolRaw(name, args);
        return "error" in out
          ? `ERROR: ${out.error}`
          : mcpResultToText(out.result);
      };

      try {
        // 1. Open a document for the agent to edit.
        const openRaw = await recordToolRaw("open_document", {});
        if ("error" in openRaw) {
          throw new Error(`open_document failed: ${openRaw.error}`);
        }
        lastDocumentId = extractDocumentId(openRaw.result);

        // 2. List MCP tools, translate to Anthropic tool schema.
        // BENCHMARK_EXCLUDED_TOOLS (module scope) is excluded here AND
        // at execution time below — see its doc comment.
        const toolList = await mcp.listTools();
        const anthropicTools = toolList.tools
          .filter((t) => !BENCHMARK_EXCLUDED_TOOLS.has(t.name))
          .map((t) => ({
            name: t.name,
            description: t.description,
            input_schema: t.inputSchema,
          })) as Array<{
          name: string;
          description?: string;
          input_schema: unknown;
          cache_control?: CacheControl;
        }>;
        // Prompt-caching breakpoint on the last tool caches the entire
        // (deterministically ordered) tool block — the largest stable
        // prefix in every request.
        if (anthropicTools.length > 0) {
          anthropicTools[anthropicTools.length - 1].cache_control = {
            type: "ephemeral",
          };
        }
        const system: Array<{
          type: "text";
          text: string;
          cache_control?: CacheControl;
        }> = [
          {
            type: "text",
            text: SYSTEM_PROMPT,
            cache_control: { type: "ephemeral" },
          },
        ];

        const userMessage = `${prompt}\n\nThe document_id for this session is "${lastDocumentId}". Use it on every tool call that requires it. Stop calling tools once you believe the document satisfies the task.`;

        // 3. Agentic loop. The kickoff message uses block form so the
        // cache-breakpoint helper can mark its last content block.
        const messages: Array<{
          role: "user" | "assistant";
          content: unknown;
        }> = [{ role: "user", content: [{ type: "text", text: userMessage }] }];

        if (config.fakeReplyForTests !== undefined) {
          // Test-only: skip the real loop, persist a hand-baked .vcad via MCP.
          // The fake reply itself is a JSON document we store via... no, simpler:
          // tests provide a stub MCP via mcpBin override. Reaching this path
          // means tests want to bypass the loop entirely; we just close out.
        } else {
          const anthropic = await makeAnthropic();
          for (let iter = 0; iter < config.maxIterations; iter++) {
            applyConversationCacheBreakpoints(messages);
            const resp = await anthropic.messages.create({
              model: config.model,
              max_tokens: config.maxOutputTokens,
              system,
              tools: anthropicTools,
              messages,
            });
            const cacheCreate = resp.usage.cache_creation_input_tokens ?? 0;
            const cacheRead = resp.usage.cache_read_input_tokens ?? 0;
            // `input_tokens` is only the uncached remainder — keep `totalIn`
            // meaning "full prompt tokens processed" so task token limits
            // stay comparable with pre-caching runs.
            totalIn += resp.usage.input_tokens + cacheCreate + cacheRead;
            totalCacheCreate += cacheCreate;
            totalCacheRead += cacheRead;
            totalOut += resp.usage.output_tokens;

            messages.push({ role: "assistant", content: resp.content });

            if (resp.stop_reason !== "tool_use") break;

            const toolResults: Array<{
              type: "tool_result";
              tool_use_id: string;
              content: AnthropicToolResultBlock[];
              is_error: boolean;
            }> = [];

            for (const block of resp.content) {
              if (block.type !== "tool_use") continue;
              const tu = block as {
                type: "tool_use";
                id: string;
                name: string;
                input: Record<string, unknown>;
              };
              // Defense in depth: excluded tools are filtered from the
              // advertised tool list, but a hallucinated tool_use block
              // must not reach MCP either.
              if (BENCHMARK_EXCLUDED_TOOLS.has(tu.name)) {
                trace.push({
                  n: nextN++,
                  tool: tu.name,
                  args: tu.input,
                  result_kind: "err",
                  wallclock_ms: 0,
                });
                toolResults.push({
                  type: "tool_result",
                  tool_use_id: tu.id,
                  content: [
                    {
                      type: "text",
                      text: `ERROR: ${tu.name} is not available during benchmark runs.`,
                    },
                  ],
                  is_error: true,
                });
                continue;
              }
              const out = await recordToolRaw(tu.name, tu.input);
              if ("error" in out) {
                toolResults.push({
                  type: "tool_result",
                  tool_use_id: tu.id,
                  content: [{ type: "text", text: `ERROR: ${out.error}` }],
                  is_error: true,
                });
              } else {
                toolResults.push({
                  type: "tool_result",
                  tool_use_id: tu.id,
                  content: mcpResultToAnthropicContent(out.result),
                  is_error: out.result.isError === true,
                });
              }
            }
            messages.push({ role: "user", content: toolResults });
          }
        }

        // 4. Read the final document from the session.
        const docRaw = await recordToolRaw("get_document", {
          document_id: lastDocumentId,
        });
        if ("error" in docRaw) {
          throw new Error(`get_document failed: ${docRaw.error}`);
        }
        if (docRaw.result.isError) {
          throw new Error(
            `get_document failed: ${mcpResultToText(docRaw.result)}`,
          );
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

export const claudeMcpSolver = makeClaudeMcpSolver();

// Test seam: re-export for unit tests. (extractDocumentId and
// extractVcadJson are exported at their declarations.)
export { mcpResultToText };
// Suppress unused-import warning for the type-only Anthropic import
// (kept for future direct typing).
export type _Anthropic = typeof Anthropic;
