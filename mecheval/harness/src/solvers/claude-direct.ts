// Direct-prompt Claude solver: one API call, model returns a .vcad JSON
// in a fenced code block. No MCP loop, no tool use — the agentic
// MCP-driven solver lands separately.
//
// This exists to get a real frontier-model number on the board fast.
// It's deliberately the worst version of the agent — single shot, no
// tool calls, no error correction, no inspection. Anything it scores
// is a floor.

import type { Solver, SolverOutput, ToolCall } from "../solver.js";
import type { Task } from "../task.js";

export interface ClaudeDirectConfig {
  model: string;
  maxOutputTokens: number;
  /** Override for tests / dry-runs — when present, returns this string
   *  instead of calling the API. */
  fakeReplyForTests?: string;
}

export const DEFAULT_CLAUDE_DIRECT: ClaudeDirectConfig = {
  model: "claude-opus-4-7",
  maxOutputTokens: 8000,
};

const SYSTEM_PROMPT = `You are a CAD modeling agent for vcad. You receive an engineering task and output exactly one .vcad document — a JSON file describing a parametric CAD scene.

The .vcad format:

{
  "version": "0.1",
  "nodes": { "<id>": { "id": <id>, "name": "<name>", "op": <CsgOp> }, ... },
  "materials": {},
  "part_materials": {},
  "roots": [{ "root": <node_id>, "material": "default" }]
}

CsgOp variants you can use:
- {"type":"Cube", "size": {"x":sx,"y":sy,"z":sz}}                   → corner at origin, extends to (sx,sy,sz)
- {"type":"Cylinder", "radius":r, "height":h, "segments":32}        → axis along Z, base on z=0
- {"type":"Sphere", "radius":r, "segments":32}                      → centered at origin
- {"type":"Cone", "radius_bottom":rb, "radius_top":rt, "height":h, "segments":32}
- {"type":"Translate", "child":<id>, "offset":{"x":dx,"y":dy,"z":dz}}
- {"type":"Rotate", "child":<id>, "axis":{"x":ax,"y":ay,"z":az}, "angle":radians}
- {"type":"Difference", "left":<id>, "right":<id>}                  → subtract right from left
- {"type":"Union", "left":<id>, "right":<id>}
- {"type":"Intersection", "left":<id>, "right":<id>}

Conventions:
- Z is up. Cube and cylinder primitives have their corner / base at the origin.
- Units are millimeters (mm).
- Output a single solid (one root) unless the task says otherwise.
- For a centered cube of side s, translate the cube by (-s/2, -s/2, -s/2).
- For holes, use a Difference: subtract a cylinder from the body.

Output format:
- Output ONLY a single \`\`\`json fenced code block containing the full .vcad document.
- No prose before or after.
- No markdown headers, no explanations.`;

/** Extract the first ```json fenced code block from the model's reply. */
export function extractVcadJson(reply: string): string {
  const fenced = /```(?:json)?\s*\n([\s\S]*?)\n```/m.exec(reply);
  if (fenced) return fenced[1].trim();
  // Fallback: take the largest balanced { ... } substring.
  const start = reply.indexOf("{");
  if (start < 0) {
    throw new Error(
      "Claude reply contained no JSON object. First 200 chars: " +
        JSON.stringify(reply.slice(0, 200)),
    );
  }
  let depth = 0;
  for (let i = start; i < reply.length; i++) {
    if (reply[i] === "{") depth++;
    else if (reply[i] === "}") {
      depth--;
      if (depth === 0) return reply.slice(start, i + 1);
    }
  }
  throw new Error("Claude reply contained an unbalanced JSON object.");
}

/** Build the user-side prompt. The system message is shared. */
export function buildUserPrompt(task: Task, taskPrompt: string): string {
  return `Task ID: ${task.id}
Tier: ${task.tier}

${taskPrompt}

Output the .vcad now.`;
}

/** Build a Solver bound to the given config. */
export function makeClaudeDirectSolver(
  cfg: Partial<ClaudeDirectConfig> = {},
): Solver {
  const config: ClaudeDirectConfig = { ...DEFAULT_CLAUDE_DIRECT, ...cfg };
  return {
    id: `claude-direct-${config.model}`,
    name: `Claude (direct, ${config.model})`,
    provider: "anthropic",
    params: { mode: "direct", model: config.model, max_tokens: config.maxOutputTokens },
    async solve(task, prompt): Promise<SolverOutput> {
      const start = performance.now();
      const userPrompt = buildUserPrompt(task, prompt);

      let reply: string;
      let tokens = { input: 0, output: 0, total: 0 };

      if (config.fakeReplyForTests !== undefined) {
        reply = config.fakeReplyForTests;
      } else {
        const apiKey = process.env.ANTHROPIC_API_KEY;
        if (!apiKey) {
          throw new Error(
            "claude-direct solver requires ANTHROPIC_API_KEY in the environment.",
          );
        }
        const { default: Anthropic } = await import("@anthropic-ai/sdk");
        const client = new Anthropic({ apiKey });
        const resp = await client.messages.create({
          model: config.model,
          max_tokens: config.maxOutputTokens,
          system: SYSTEM_PROMPT,
          messages: [{ role: "user", content: userPrompt }],
        });
        reply = resp.content
          .flatMap((b) => (b.type === "text" ? [b.text] : []))
          .join("\n");
        tokens = {
          input: resp.usage.input_tokens,
          output: resp.usage.output_tokens,
          total: resp.usage.input_tokens + resp.usage.output_tokens,
        };
      }

      const vcadJson = extractVcadJson(reply);
      const wallclockSec = (performance.now() - start) / 1000;
      const toolCall: ToolCall = {
        n: 0,
        tool: "anthropic.messages.create",
        args: {
          model: config.model,
          max_tokens: config.maxOutputTokens,
          system_chars: SYSTEM_PROMPT.length,
          user_chars: userPrompt.length,
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

export const claudeDirectSolver = makeClaudeDirectSolver();
