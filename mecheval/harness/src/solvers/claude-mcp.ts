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

interface MinimalMcpClient {
  listTools(): Promise<{ tools: Array<{ name: string; description?: string; inputSchema: unknown }> }>;
  callTool(args: { name: string; arguments: Record<string, unknown> }): Promise<{
    content?: Array<{ type: string; text?: string }>;
    isError?: boolean;
  }>;
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
function mcpResultToText(result: {
  content?: Array<{ type: string; text?: string }>;
  isError?: boolean;
}): string {
  const parts = (result.content ?? []).map((c) =>
    c.type === "text" && c.text != null ? c.text : `[${c.type} content]`,
  );
  const body = parts.join("\n");
  return result.isError ? `ERROR: ${body}` : body;
}

function extractDocumentId(openResult: {
  content?: Array<{ type: string; text?: string }>;
}): string {
  const text = mcpResultToText(openResult);
  try {
    const parsed = JSON.parse(text);
    if (typeof parsed.document_id === "string") return parsed.document_id;
  } catch {
    // fall through
  }
  throw new Error(`open_document did not return a JSON document_id; got: ${text}`);
}

interface AnthropicLite {
  messages: {
    create(req: {
      model: string;
      max_tokens: number;
      system: string;
      tools: Array<{ name: string; description?: string; input_schema: unknown }>;
      messages: Array<{ role: "user" | "assistant"; content: unknown }>;
    }): Promise<{
      stop_reason: string | null;
      content: Array<
        | { type: "text"; text: string }
        | { type: "tool_use"; id: string; name: string; input: Record<string, unknown> }
        | { type: string; [k: string]: unknown }
      >;
      usage: { input_tokens: number; output_tokens: number };
    }>;
  };
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
      let lastDocumentId: string | null = null;

      const recordTool = async (
        name: string,
        args: Record<string, unknown>,
      ): Promise<string> => {
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
          return mcpResultToText(result);
        } catch (e) {
          const ms = performance.now() - t0;
          trace.push({
            n: nextN++,
            tool: name,
            args,
            result_kind: "err",
            wallclock_ms: ms,
          });
          return `ERROR: ${(e as Error).message}`;
        }
      };

      try {
        // 1. Open a document for the agent to edit.
        const openText = await recordTool("open_document", {});
        lastDocumentId = extractDocumentId({
          content: [{ type: "text", text: openText.replace(/^ERROR: /, "") }],
        });

        // 2. List MCP tools, translate to Anthropic tool schema.
        // The self-grading oracle (verify_part / list_eval_tasks) is
        // excluded during benchmark runs — letting the model grade itself
        // against the task's own checks mid-run would contaminate the
        // leaderboard. render_view stays: eyes are product surface.
        const ORACLE_TOOLS = new Set(["verify_part", "list_eval_tasks"]);
        const toolList = await mcp.listTools();
        const anthropicTools = toolList.tools
          .filter((t) => !ORACLE_TOOLS.has(t.name))
          .map((t) => ({
            name: t.name,
            description: t.description,
            input_schema: t.inputSchema,
          }));

        const userMessage = `${prompt}\n\nThe document_id for this session is "${lastDocumentId}". Use it on every tool call that requires it. Stop calling tools once you believe the document satisfies the task.`;

        // 3. Agentic loop.
        const messages: Array<{
          role: "user" | "assistant";
          content: unknown;
        }> = [{ role: "user", content: userMessage }];

        if (config.fakeReplyForTests !== undefined) {
          // Test-only: skip the real loop, persist a hand-baked .vcad via MCP.
          // The fake reply itself is a JSON document we store via... no, simpler:
          // tests provide a stub MCP via mcpBin override. Reaching this path
          // means tests want to bypass the loop entirely; we just close out.
        } else {
          const anthropic = await makeAnthropic();
          for (let iter = 0; iter < config.maxIterations; iter++) {
            const resp = await anthropic.messages.create({
              model: config.model,
              max_tokens: config.maxOutputTokens,
              system: SYSTEM_PROMPT,
              tools: anthropicTools,
              messages,
            });
            totalIn += resp.usage.input_tokens;
            totalOut += resp.usage.output_tokens;

            messages.push({ role: "assistant", content: resp.content });

            if (resp.stop_reason !== "tool_use") break;

            const toolResults: Array<{
              type: "tool_result";
              tool_use_id: string;
              content: Array<{ type: "text"; text: string }>;
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
              const text = await recordTool(tu.name, tu.input);
              toolResults.push({
                type: "tool_result",
                tool_use_id: tu.id,
                content: [{ type: "text", text }],
                is_error: text.startsWith("ERROR:"),
              });
            }
            messages.push({ role: "user", content: toolResults });
          }
        }

        // 4. Read the final document from the session.
        const docText = await recordTool("get_document", {
          document_id: lastDocumentId,
        });
        let vcadJson: string;
        try {
          // get_document returns the IR as JSON-text. Pretty-print it so the
          // run blob's persisted .vcad is readable.
          const parsed = JSON.parse(docText);
          vcadJson = JSON.stringify(parsed, null, 2);
        } catch {
          vcadJson = docText;
        }

        const wallclockSec = (performance.now() - start) / 1000;
        return {
          vcadJson,
          controlPolicy: null,
          toolCalls: trace,
          tokens: {
            input: totalIn,
            output: totalOut,
            total: totalIn + totalOut,
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

// Test seam: re-export for unit tests.
export { mcpResultToText, extractDocumentId };
// Suppress unused-import warning for the type-only Anthropic import
// (kept for future direct typing).
export type _Anthropic = typeof Anthropic;
