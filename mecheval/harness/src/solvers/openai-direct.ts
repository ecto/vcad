// Direct-prompt OpenAI solver: one API call, model returns a .vcad JSON
// in a fenced code block. Mirrors the claude-direct contract so the
// leaderboard / pass^k aggregation treats it identically.
//
// Provider tag: "openai". Solver IDs follow `openai-direct-<model>`.

import type { AgentAttachment, Solver, SolverOutput, ToolCall } from "../solver.js";
import type { Task } from "../task.js";
import { SYSTEM_PROMPT, buildUserPrompt, extractVcadJson } from "./claude-direct.js";

type OpenAiUserContent =
  | { type: "text"; text: string }
  | { type: "image_url"; image_url: { url: string } };

/**
 * Convert agent attachments into OpenAI chat-completion user content.
 * Images are inlined as data URLs (the chat-completions endpoint does
 * not yet accept base64 image_url objects directly across all models).
 */
export function buildOpenAiContent(
  task: Task,
  taskPrompt: string,
  attachments: AgentAttachment[],
): OpenAiUserContent[] {
  const blocks: OpenAiUserContent[] = [];
  for (const a of attachments) {
    if (a.kind === "reference_image") {
      blocks.push({
        type: "image_url",
        image_url: { url: `data:${a.mime};base64,${a.base64}` },
      });
      const view = (a.meta.view as string | undefined) ?? "view";
      blocks.push({
        type: "text",
        text: `↑ ${view} view (image_kind: ${a.meta.image_kind ?? "unspecified"})`,
      });
    } else if (a.kind === "known_dimensions") {
      blocks.push({ type: "text", text: `Stated dimension: ${a.text}` });
    }
  }
  blocks.push({ type: "text", text: buildUserPrompt(task, taskPrompt) });
  return blocks;
}

export interface OpenAiDirectConfig {
  model: string;
  maxOutputTokens: number;
  /** Override for tests / dry-runs. */
  fakeReplyForTests?: string;
}

export const DEFAULT_OPENAI_DIRECT: OpenAiDirectConfig = {
  model: "gpt-5",
  maxOutputTokens: 8000,
};

/** Build a Solver bound to the given config. */
export function makeOpenAiDirectSolver(
  cfg: Partial<OpenAiDirectConfig> = {},
): Solver {
  const config: OpenAiDirectConfig = { ...DEFAULT_OPENAI_DIRECT, ...cfg };
  return {
    id: `openai-direct-${config.model}`,
    name: `OpenAI (direct, ${config.model})`,
    provider: "openai",
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
        const apiKey = process.env.OPENAI_API_KEY;
        if (!apiKey) {
          throw new Error(
            "openai-direct solver requires OPENAI_API_KEY in the environment.",
          );
        }
        const { default: OpenAI } = await import("openai");
        const client = new OpenAI({ apiKey });

        // GPT-5 family + reasoning models (o1/o3) take `max_completion_tokens`
        // and reject the legacy `max_tokens` field. Older chat models still
        // use `max_tokens`. We branch by model id prefix — same heuristic
        // OpenAI's own docs use.
        const isReasoning = /^(gpt-5|o1|o3|o4)/.test(config.model);
        const tokenField = isReasoning
          ? { max_completion_tokens: config.maxOutputTokens }
          : { max_tokens: config.maxOutputTokens };

        const resp = await client.chat.completions.create({
          model: config.model,
          messages: [
            { role: "system", content: SYSTEM_PROMPT },
            // SDK type for `content` varies across versions; the wire
            // shape we build matches the chat-completions multimodal spec.
            { role: "user", content: content as never },
          ],
          ...tokenField,
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
        tool: "openai.chat.completions.create",
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

export const openAiDirectSolver = makeOpenAiDirectSolver();
