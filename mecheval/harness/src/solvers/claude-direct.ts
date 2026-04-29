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

export const SYSTEM_PROMPT = `You are a CAD modeling agent for vcad. You receive an engineering task and output exactly one .vcad document — a JSON file describing a parametric CAD scene.

The .vcad format. **Node ids are integers** (u64). The keys of the \`nodes\` map are the JSON-string form of those integers (\`"1"\`, \`"2"\`, …) — never alphabetic names like \`"cube"\` or \`"base"\`. The \`id\` field inside each node, and any \`child\` / \`left\` / \`right\` / \`root\` reference, is the raw integer (no quotes).

Concrete example — a 10mm cube with a 3mm hole drilled through it:

{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "cube",    "op": { "type": "Cube", "size": { "x": 10, "y": 10, "z": 10 } } },
    "2": { "id": 2, "name": "drill",   "op": { "type": "Cylinder", "radius": 1.5, "height": 12, "segments": 32 } },
    "3": { "id": 3, "name": "drill_p", "op": { "type": "Translate", "child": 2, "offset": { "x": 5, "y": 5, "z": -1 } } },
    "4": { "id": 4, "name": "result",  "op": { "type": "Difference", "left": 1, "right": 3 } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [{ "root": 4, "material": "default" }]
}

CsgOp variants you can use:
- {"type":"Cube", "size": {"x":sx,"y":sy,"z":sz}}                   → corner at origin, extends to (sx,sy,sz)
- {"type":"Cylinder", "radius":r, "height":h, "segments":32}        → axis along Z, base on z=0
- {"type":"Sphere", "radius":r, "segments":32}                      → centered at origin
- {"type":"Cone", "radius_bottom":rb, "radius_top":rt, "height":h, "segments":32}
- {"type":"Translate", "child":<id>, "offset":{"x":dx,"y":dy,"z":dz}}
- {"type":"Rotate", "child":<id>, "angles":{"x":rx,"y":ry,"z":rz}}  → Euler angles in DEGREES around the X, Y, Z axes (intrinsic, applied X→Y→Z). The field is named "angles" and is a vector — *not* an OpenSCAD-style {axis, angle}. To rotate a child 90° about Y: {"type":"Rotate","child":<id>,"angles":{"x":0,"y":90,"z":0}}.
- {"type":"Difference", "left":<id>, "right":<id>}                  → subtract right from left
- {"type":"Union", "left":<id>, "right":<id>}
- {"type":"Intersection", "left":<id>, "right":<id>}

Conventions:
- Z is up. Cube and cylinder primitives have their corner / base at the origin.
- Units are millimeters (mm) for Suites A/B; **meters** for Suite C (mech) — a Suite C task will say so explicitly.
- Output a single solid (one root) unless the task says otherwise.
- For a centered cube of side s, translate the cube by (-s/2, -s/2, -s/2).
- For holes, use a Difference: subtract a cylinder from the body.

Suite C (mech) — additional fields. Only emit these when the task is in Suite C (mech / robotics). The same root \`nodes\` graph defines per-part geometry; Suite C also requires:

- \`partDefs\`: { "<partDefId>": { "id": "<partDefId>", "name": "<label>", "root": <node_id> } } — each part definition points at a root node in \`nodes\`.
- \`instances\`: [ { "id": "<instId>", "partDefId": "<partDefId>", "name": "<label>", "tags": ["tip"], "transform": { "translation": {"x":..,"y":..,"z":..}, "rotation": {"x":..,"y":..,"z":..}, "scale": {"x":1,"y":1,"z":1} } } ] — concrete links in the assembly. Use \`tags\` to mark the end-effector (\`["tip"]\`), the base (\`["base"]\`), feet (\`["foot_left"]\`), etc. Suite C graders read these.
- \`joints\`: [ { "id": "<jointId>", "parentInstanceId": "<instId>", "childInstanceId": "<instId>", "parentAnchor": {"x":..,"y":..,"z":..}, "childAnchor": {"x":..,"y":..,"z":..}, "kind": { "type": "Revolute", "axis": {"x":0,"y":0,"z":1}, "limits": [-90, 90] }, "state": 0 } ] — joint kinds: \`Revolute\` (axis + degree limits), \`Slider\` (axis + mm limits), \`Cylindrical\` (axis), \`Ball\`, \`Fixed\`. Anchors are local to each instance, in the same units as the document.
- \`groundInstanceId\`: the id of the instance fixed in world space (the base).

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
