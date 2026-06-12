/**
 * Guards that keep scored benchmark runs honest:
 *
 * 1. BENCHMARK_EXCLUDED_TOOLS — close_document destroyed ~3% of attempts
 *    in the 2026-06 matrix (agents called it mid-run; the final
 *    get_document then found no session). It and the self-grading oracle
 *    must never be advertised to, or executed for, a benchmark agent.
 *
 * 2. computeLimitsExceeded — task limits (max_tokens, max_wallclock_sec,
 *    max_tool_calls) were declared in every task JSON but never checked;
 *    the harness now annotates summary.limits_exceeded. Annotation only:
 *    `passed` is not flipped (scoring policy stays a publication-time
 *    decision and historical comparability holds).
 */

import { describe, it, expect } from "vitest";
import { BENCHMARK_EXCLUDED_TOOLS } from "../solvers/claude-mcp.js";
import { buildBlob, computeLimitsExceeded } from "../blob.js";
import type { PartialGraderBlob } from "../blob.js";
import type { Task } from "../task.js";
import type { Solver, SolverOutput } from "../solver.js";

describe("BENCHMARK_EXCLUDED_TOOLS", () => {
  it("excludes the session-destroying and oracle tools", () => {
    expect(BENCHMARK_EXCLUDED_TOOLS.has("close_document")).toBe(true);
    expect(BENCHMARK_EXCLUDED_TOOLS.has("verify_part")).toBe(true);
    expect(BENCHMARK_EXCLUDED_TOOLS.has("list_eval_tasks")).toBe(true);
  });

  it("keeps the tools the agentic loop depends on", () => {
    for (const essential of [
      "open_document",
      "get_document",
      "create_cad_loon",
      "inspect_cad",
      "render_view",
    ]) {
      expect(BENCHMARK_EXCLUDED_TOOLS.has(essential)).toBe(false);
    }
  });
});

function output(
  over: Partial<{ total: number; wallclockSec: number; calls: number }> = {},
): Pick<SolverOutput, "tokens" | "wallclockSec" | "toolCalls"> {
  const calls = over.calls ?? 3;
  return {
    tokens: { input: 0, output: 0, total: over.total ?? 1000 },
    wallclockSec: over.wallclockSec ?? 10,
    toolCalls: Array.from({ length: calls }, (_, n) => ({
      n,
      tool: "create",
      args: {},
      result_kind: "ok" as const,
      wallclock_ms: 1,
    })),
  };
}

const LIMITS = { max_tokens: 20_000, max_wallclock_sec: 120, max_tool_calls: 20 };

describe("computeLimitsExceeded", () => {
  it("returns empty when under every limit", () => {
    expect(computeLimitsExceeded(LIMITS, output())).toEqual([]);
  });

  it("returns empty when the task declares no limits", () => {
    expect(
      computeLimitsExceeded(undefined, output({ total: 10_000_000 })),
    ).toEqual([]);
  });

  it("flags each exceeded ceiling with measured vs declared values", () => {
    const exceeded = computeLimitsExceeded(
      LIMITS,
      output({ total: 178_292, wallclockSec: 301.4, calls: 31 }),
    );
    expect(exceeded).toEqual([
      "max_tokens: 178292 > 20000",
      "max_wallclock_sec: 301 > 120",
      "max_tool_calls: 31 > 20",
    ]);
  });

  it("flags a single ceiling independently", () => {
    expect(computeLimitsExceeded(LIMITS, output({ total: 25_000 }))).toEqual([
      "max_tokens: 25000 > 20000",
    ]);
  });
});

describe("buildBlob limit annotation", () => {
  const partial: PartialGraderBlob = {
    schema_version: 0,
    task_id: "a1-block-01",
    task_sha256: "deadbeef",
    checks: [],
    summary: {
      passed: true,
      checks_passed: 4,
      checks_total: 4,
      score: 1,
      anti_cheese_violated: false,
      limits_exceeded: ["grader-side-entry"],
    },
  };
  const task = { id: "a1-block-01", limits: LIMITS } as unknown as Task;
  const solver: Solver = {
    id: "test-solver",
    name: "Test",
    provider: "test",
    params: {},
    solve: async () => {
      throw new Error("unused");
    },
  };

  function blobWith(out: ReturnType<typeof output>) {
    return buildBlob({
      partial,
      task,
      solver,
      solverOutput: {
        vcadJson: "{}",
        controlPolicy: null,
        ...out,
      } as SolverOutput,
      prompt: { seed: "a1-block-01", rendered: "make a block", attachments: [] },
      vcadPath: "x.vcad",
      vcadSha256: "deadbeef",
      runId: "20260611T000000Z-0000",
      startedAt: new Date(0),
      endedAt: new Date(1000),
      harnessVersion: "0.0.1",
      submissionKind: "self-run",
    });
  }

  it("appends harness violations after grader entries without flipping passed", () => {
    const blob = blobWith(output({ total: 178_292 }));
    expect(blob.summary.limits_exceeded).toEqual([
      "grader-side-entry",
      "max_tokens: 178292 > 20000",
    ]);
    expect(blob.summary.passed).toBe(true);
  });

  it("leaves limits_exceeded untouched when under limits", () => {
    const blob = blobWith(output());
    expect(blob.summary.limits_exceeded).toEqual(["grader-side-entry"]);
  });
});
