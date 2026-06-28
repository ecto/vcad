// Direct-prompt Wafer solver: one OpenAI-compatible API call against
// wafer.ai's serverless endpoint (https://pass.wafer.ai/v1), returning a
// .vcad JSON in a fenced code block. Defaults to GLM-5.2. Mirrors the
// openai-direct contract so the leaderboard / pass^k aggregation treats it
// identically — it reuses openai-direct's multimodal content builder.
//
// Provider tag: "wafer". Solver IDs follow `wafer-direct-<model>`.

import type { AgentAttachment, Solver, SolverOutput, ToolCall } from "../solver.js";
import type { Task } from "../task.js";
import { SYSTEM_PROMPT, buildUserPrompt, extractVcadJson } from "./claude-direct.js";
import { buildOpenAiContent } from "./openai-direct.js";

/** wafer.ai serverless OpenAI-compatible base URL (auth: `Bearer wfr_…`). */
const WAFER_BASE_URL = "https://pass.wafer.ai/v1";

export interface WaferDirectConfig {
  model: string;
  maxOutputTokens: number;
  /** Override the OpenAI-compatible base URL (defaults to wafer.ai serverless). */
  baseUrl: string;
  /** Override for tests / dry-runs. */
  fakeReplyForTests?: string;
}

export const DEFAULT_WAFER_DIRECT: WaferDirectConfig = {
  model: "GLM-5.2",
  maxOutputTokens: 8000,
  baseUrl: WAFER_BASE_URL,
};

/** Build a Solver bound to the given config. */
export function makeWaferDirectSolver(
  cfg: Partial<WaferDirectConfig> = {},
): Solver {
  const config: WaferDirectConfig = { ...DEFAULT_WAFER_DIRECT, ...cfg };
  return {
    id: `wafer-direct-${config.model}`,
    name: `Wafer (direct, ${config.model})`,
    provider: "wafer",
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
        const apiKey = process.env.WAFER_API_KEY;
        if (!apiKey) {
          throw new Error(
            "wafer-direct solver requires WAFER_API_KEY in the environment.",
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
        tool: "wafer.chat.completions.create",
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

export const waferDirectSolver = makeWaferDirectSolver();
