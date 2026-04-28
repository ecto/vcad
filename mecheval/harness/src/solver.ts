// Solver interface — the contract a submission must implement.
//
// A solver receives a Task (already loaded) plus the rendered prompt, and
// returns a .vcad output (as a JSON string) plus the trace of what it did.
// Real solvers will drive an LLM through the vcad MCP server. The skeleton
// ships one stub solver — DEFAULT_CUBE — that always returns a 50mm cube.
// It's the project mascot villain: any submission worth its salt must beat
// it.

import type { Task } from "./task.js";

export interface ToolCall {
  n: number;
  tool: string;
  args: unknown;
  result_kind: "ok" | "err";
  wallclock_ms: number;
}

export interface SolverOutput {
  vcadJson: string;
  controlPolicy: string | null;
  toolCalls: ToolCall[];
  tokens: { input: number; output: number; total: number };
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
  /** Run the solver on one task. */
  solve(task: Task, prompt: string): Promise<SolverOutput>;
}

// ---- DEFAULT_CUBE — the villain stub ----------------------------------

/** Returns a 50mm cube centered at origin, regardless of prompt. */
export const defaultCubeSolver: Solver = {
  id: "default-cube",
  name: "DEFAULT_CUBE (baseline villain)",
  provider: "stub",
  params: { size_mm: 50 },
  async solve(_task, _prompt): Promise<SolverOutput> {
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

/** Look up a solver by id. Currently ships:
 *  - default-cube           — baseline villain
 *  - claude-direct[-<m>]    — single-shot, prompt-only
 *  - claude-mcp[-<m>]       — agentic, drives @vcad/mcp via MCP tool loop
 *
 *  SDKs (Anthropic, MCP) are loaded lazily inside `solve()`, so callers
 *  that only use DEFAULT_CUBE never pay for the import. */
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
  throw new Error(
    `unknown solver "${id}". Available: "default-cube", "claude-direct[-<m>]", "claude-mcp[-<m>]".`,
  );
}
