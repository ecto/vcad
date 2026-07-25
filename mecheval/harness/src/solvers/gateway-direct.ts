// Direct-prompt Vercel AI Gateway solver: one OpenAI-compatible API call
// against https://ai-gateway.vercel.sh/v1, returning a .vcad JSON in a
// fenced code block. The gateway fronts every major provider behind one
// endpoint and one key, so the roster can grow without a new solver per
// provider. Models are addressed as "<provider>/<model>" (e.g.
// "anthropic/claude-sonnet-4.5", "xai/grok-4", "google/gemini-2.5-pro").
//
// Solver IDs follow `gateway-direct-<provider>-<model>` — the "/" in the
// gateway model slug is folded to "-" because model ids become run-blob
// directory names and leaderboard URLs. The original slug is preserved in
// `params.model` for forensics and for the API call itself.
//
// Provider tag: the gateway slug's provider segment (so the leaderboard's
// provider marks and brand colors keep working).

import type { AgentAttachment, Solver, SolverOutput, ToolCall } from "../solver.js";
import type { Task } from "../task.js";
import { SYSTEM_PROMPT, buildUserPrompt, extractVcadJson } from "./claude-direct.js";
import { buildOpenAiContent } from "./openai-direct.js";

/** Vercel AI Gateway OpenAI-compatible base URL (auth: `Bearer vck_…`). */
const GATEWAY_BASE_URL = "https://ai-gateway.vercel.sh/v1";

export interface GatewayDirectConfig {
  /** Gateway model slug, "<provider>/<model>". */
  model: string;
  maxOutputTokens: number;
  /** Override the OpenAI-compatible base URL (defaults to the gateway). */
  baseUrl: string;
  /** Override for tests / dry-runs. */
  fakeReplyForTests?: string;
}

export const DEFAULT_GATEWAY_DIRECT: GatewayDirectConfig = {
  model: "anthropic/claude-sonnet-4.5",
  maxOutputTokens: 8000,
  baseUrl: GATEWAY_BASE_URL,
};

/** Fold a gateway model slug into a filesystem/URL-safe id segment. */
export function gatewayIdSegment(model: string): string {
  return model.replace(/\//g, "-");
}

/** Build a Solver bound to the given config. */
export function makeGatewayDirectSolver(
  cfg: Partial<GatewayDirectConfig> = {},
): Solver {
  const config: GatewayDirectConfig = { ...DEFAULT_GATEWAY_DIRECT, ...cfg };
  const provider = config.model.includes("/")
    ? config.model.split("/", 1)[0]
    : "gateway";
  return {
    id: `gateway-direct-${gatewayIdSegment(config.model)}`,
    name: `AI Gateway (direct, ${config.model})`,
    provider,
    params: { mode: "direct", model: config.model, max_tokens: config.maxOutputTokens },
    async solve(
      task: Task,
      prompt: string,
      attachments: AgentAttachment[] = [],
    ): Promise<SolverOutput> {
      const start = performance.now();
      const userPrompt = buildUserPrompt(task, prompt);
      const content = buildOpenAiContent(task, prompt, attachments);
      const imageCount = attachments.filter((a) => a.kind === "reference_image").length;

      let reply: string;
      let tokens = { input: 0, output: 0, total: 0 };

      if (config.fakeReplyForTests !== undefined) {
        reply = config.fakeReplyForTests;
      } else {
        const apiKey = process.env.AI_GATEWAY_API_KEY;
        if (!apiKey) {
          throw new Error(
            "gateway-direct solver requires AI_GATEWAY_API_KEY in the environment.",
          );
        }
        const { default: OpenAI } = await import("openai");
        const client = new OpenAI({ apiKey, baseURL: config.baseUrl });

        const resp = await client.chat.completions.create({
          model: config.model,
          messages: [
            { role: "system", content: SYSTEM_PROMPT },
            // SDK type for `content` varies across versions; the wire
            // shape we build matches the chat-completions multimodal spec.
            { role: "user", content: content as never },
          ],
          max_tokens: config.maxOutputTokens,
        });
        reply = resp.choices[0]?.message?.content ?? "";
        tokens = {
          input: resp.usage?.prompt_tokens ?? 0,
          output: resp.usage?.completion_tokens ?? 0,
          total: resp.usage?.total_tokens ?? 0,
        };
      }

      const vcadJson = extractVcadJson(reply);
      const wallclockSec = (performance.now() - start) / 1000;
      const toolCall: ToolCall = {
        n: 0,
        tool: "gateway.chat.completions.create",
        args: {
          model: config.model,
          max_tokens: config.maxOutputTokens,
          system_chars: SYSTEM_PROMPT.length,
          user_chars: userPrompt.length,
          image_attachments: imageCount,
        },
        result_kind: "ok",
        wallclock_ms: wallclockSec * 1000,
      };

      return {
        vcadJson,
        controlPolicy: null,
        toolCalls: [toolCall],
        tokens,
        wallclockSec,
      };
    },
  };
}

export const gatewayDirectSolver = makeGatewayDirectSolver();
