// Solver interface — the contract a submission must implement.
//
// A solver receives a Task (already loaded) plus the rendered prompt, and
// returns a .vcad output (as a JSON string) plus the trace of what it did.
// Real solvers will drive an LLM through the vcad MCP server. The skeleton
// ships one stub solver — DEFAULT_CUBE — that always returns a 50mm cube.
// It's the project mascot villain: any submission worth its salt must beat
// it.

import type { StructuredInput, Task } from "./task.js";

export interface ToolCall {
  n: number;
  tool: string;
  args: unknown;
  result_kind: "ok" | "err";
  wallclock_ms: number;
}

/**
 * One agent-visible task input, resolved by the harness before invocation.
 * `meta` carries the original task-input record (for forensics and so the
 * solver can read view labels, fiducial info, etc.).
 */
export type AgentAttachment =
  | {
      kind: "reference_image";
      meta: StructuredInput;
      mime: string;
      base64: string;
    }
  | {
      kind: "known_dimensions";
      meta: StructuredInput;
      text: string;
    }
  | {
      kind: "other";
      meta: StructuredInput;
      path?: string;
    };

export interface SolverOutput {
  vcadJson: string;
  controlPolicy: string | null;
  toolCalls: ToolCall[];
  tokens: {
    /** Full prompt tokens processed, INCLUDING cached reads/writes — same
     *  semantics as before prompt caching, so task token limits stay
     *  comparable across runs. */
    input: number;
    output: number;
    total: number;
    /** Tokens written to the prompt cache (billed ~1.25x input rate).
     *  Included in `input`; broken out for cost analysis. */
    cache_creation_input?: number;
    /** Tokens served from the prompt cache (billed ~0.1x input rate).
     *  Included in `input`; broken out for cost analysis. */
    cache_read_input?: number;
  };
  wallclockSec: number;
}

export interface Solver {
  /** Stable identifier — slug for the model + version + provider. */
  readonly id: string;
  /** Display name. */
  readonly name: string;
  /** Provider tag (`anthropic`, `openai`, `local`, …). */
  readonly provider: string;
  /** Free-form sampling/config payload, copied into the run blob. */
  readonly params: Record<string, unknown>;
  /**
   * Run the solver on one task. `attachments` are the resolved agent-
   * visible inputs (images, dimensions, etc.); solvers that don't
   * understand multimodal inputs may ignore them.
   */
  solve(
    task: Task,
    prompt: string,
    attachments?: AgentAttachment[],
  ): Promise<SolverOutput>;
}

// ---- DEFAULT_CUBE — the villain stub ----------------------------------

/** Returns a 50mm cube centered at origin, regardless of prompt. */
export const defaultCubeSolver: Solver = {
  id: "default-cube",
  name: "DEFAULT_CUBE (baseline villain)",
  provider: "stub",
  params: { size_mm: 50 },
  async solve(_task, _prompt, _attachments?): Promise<SolverOutput> {
    const start = performance.now();
    const vcad = {
      version: "0.1",
      nodes: {
        "1": {
          id: 1,
          name: "DEFAULT_CUBE",
          op: { type: "Cube", size: { x: 50.0, y: 50.0, z: 50.0 } },
        },
      },
      materials: {},
      part_materials: {},
      roots: [{ root: 1, material: "default" }],
    };
    const wallclockSec = (performance.now() - start) / 1000;
    return {
      vcadJson: JSON.stringify(vcad, null, 2),
      controlPolicy: null,
      toolCalls: [
        {
          n: 0,
          tool: "create_cad_document",
          args: { stub: "DEFAULT_CUBE" },
          result_kind: "ok",
          wallclock_ms: wallclockSec * 1000,
        },
      ],
      tokens: { input: 0, output: 0, total: 0 },
      wallclockSec,
    };
  },
};

import { claudeDirectSolver, makeClaudeDirectSolver } from "./solvers/claude-direct.js";
import { claudeMcpSolver, makeClaudeMcpSolver } from "./solvers/claude-mcp.js";
import { openAiDirectSolver, makeOpenAiDirectSolver } from "./solvers/openai-direct.js";
import { waferDirectSolver, makeWaferDirectSolver } from "./solvers/wafer-direct.js";
import { gatewayDirectSolver, makeGatewayDirectSolver } from "./solvers/gateway-direct.js";

/** Look up a solver by id. Currently ships:
 *  - default-cube           — baseline villain
 *  - claude-direct[-<m>]    — single-shot, prompt-only (Anthropic)
 *  - claude-mcp[-<m>]       — agentic, drives @vcad/mcp via MCP tool loop
 *  - openai-direct[-<m>]    — single-shot, prompt-only (OpenAI)
 *  - wafer-direct[-<m>]     — single-shot, prompt-only (wafer.ai, default GLM-5.2)
 *  - gateway-direct[-<p>/<m>] — single-shot, prompt-only via Vercel AI Gateway;
 *    model is the gateway slug ("anthropic/claude-sonnet-4.5", "xai/grok-4", …)
 *    and may be given with "/" or "-" as the separator
 *
 *  SDKs (Anthropic, MCP, OpenAI) are loaded lazily inside `solve()`, so
 *  callers that only use DEFAULT_CUBE never pay for the import. */
export function getSolver(id: string): Solver {
  if (id === "default-cube" || id === "DEFAULT_CUBE") return defaultCubeSolver;
  if (id === "claude-direct") return claudeDirectSolver;
  if (id.startsWith("claude-direct-")) {
    return makeClaudeDirectSolver({ model: id.slice("claude-direct-".length) });
  }
  if (id === "claude-mcp") return claudeMcpSolver;
  if (id.startsWith("claude-mcp-")) {
    return makeClaudeMcpSolver({ model: id.slice("claude-mcp-".length) });
  }
  if (id === "openai-direct") return openAiDirectSolver;
  if (id.startsWith("openai-direct-")) {
    return makeOpenAiDirectSolver({ model: id.slice("openai-direct-".length) });
  }
  if (id === "wafer-direct") return waferDirectSolver;
  if (id.startsWith("wafer-direct-")) {
    return makeWaferDirectSolver({ model: id.slice("wafer-direct-".length) });
  }
  if (id === "gateway-direct") return gatewayDirectSolver;
  if (id.startsWith("gateway-direct-")) {
    let model = id.slice("gateway-direct-".length);
    // Accept both the gateway's native "provider/model" slug and the
    // filesystem-safe "provider-model" spelling (first "-" → "/").
    if (!model.includes("/")) model = model.replace("-", "/");
    return makeGatewayDirectSolver({ model });
  }
  throw new Error(
    `unknown solver "${id}". Available: "default-cube", "claude-direct[-<m>]", "claude-mcp[-<m>]", "openai-direct[-<m>]", "wafer-direct[-<m>]", "gateway-direct[-<provider>/<model>]".`,
  );
}
