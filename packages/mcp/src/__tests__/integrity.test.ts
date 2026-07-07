/**
 * Mutation integrity metrics (torr session-3 meta-fix): every mutation
 * response must carry volume / bbox / CoM / watertightness — and, for
 * documents containing a circular pattern, the CoM's distance from the
 * pattern axis — so silently corrupt geometry is visible at creation time
 * instead of only via an out-of-band inspect_cad.
 */

import { beforeAll, describe, expect, it } from "vitest";
import { Engine, getKernelWasm } from "@vcad/engine";
import { commandRegistry } from "@vcad/core";
import { computeIntegrity } from "../tools/integrity.js";
import { dispatchRegistryTool } from "../tools/registry-dispatch.js";
import { registerSession } from "../tools/session.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
  // Wire the kernel WASM into the registry — required for the `create`
  // planner path, same bootstrap createServer does at startup.
  const wasm = (await getKernelWasm()) as unknown as Record<string, unknown>;
  const getToolSchemas = wasm.get_tool_schemas as (() => string) | undefined;
  if (getToolSchemas) commandRegistry.loadSchemas(getToolSchemas());
  const getAnthropicToolsJson = wasm.get_anthropic_tools_json as
    | (() => string)
    | undefined;
  const buildChatSystemPrompt = wasm.build_chat_system_prompt as
    | ((parts: string, sel: string) => string)
    | undefined;
  const planChatTool = wasm.plan_chat_tool as
    | ((tool: string, args: string, doc: string) => string)
    | undefined;
  if (getAnthropicToolsJson && buildChatSystemPrompt) {
    commandRegistry.setWasm({
      get_anthropic_tools_json: getAnthropicToolsJson,
      build_chat_system_prompt: buildChatSystemPrompt,
      plan_chat_tool: planChatTool,
    });
  }
});

function docFromLoon(source: string) {
  const doc = engine.evalVcadSource(source);
  if (!doc) throw new Error("engine build lacks loon support");
  return doc;
}

describe("computeIntegrity", () => {
  it("reports a closed cube as watertight with exact metrics", () => {
    const report = computeIntegrity(docFromLoon("[cube 10 20 30]"), engine);
    expect(report).not.toBeNull();
    expect(report!.volume_mm3).toBeCloseTo(6000, 3);
    expect(report!.watertight).toBe(true);
    expect(report!.open_edges).toBe(0);
    expect(report!.warnings).toEqual([]);
    expect(report!.bounding_box).toEqual({
      min: { x: 0, y: 0, z: 0 },
      max: { x: 10, y: 20, z: 30 },
    });
    expect(report!.center_of_mass!.x).toBeCloseTo(5, 2);
    expect(report!.com_axis_distance_mm).toBeUndefined();
  });

  it("kernel wasm carries the oblique-boolean fixes (torr B1)", () => {
    // Near-containment intersection: a thin rotated blade ~99% inside a
    // cylinder, corner slivers poking out. The pre-fix kernel returned an
    // empty solid (volume 0, null bbox) — the flagship silent corruption
    // from the torr session-3 field report. Also guards against shipping
    // a stale checked-in kernel wasm.
    const report = computeIntegrity(
      docFromLoon(
        "[intersection [cylinder 45.0 13] [translate 21.50 0 0 [rotate 39.29 0 0 [cube 23.50 0.5 12.57]]]]",
      ),
      engine,
    );
    expect(report).not.toBeNull();
    expect(report!.volume_mm3).toBeGreaterThan(140);
    expect(report!.volume_mm3).toBeLessThan(148);
  });

  it("reports CoM-vs-axis distance for circular patterns", () => {
    const report = computeIntegrity(
      docFromLoon(
        "[circular-pattern 0 0 0  0 0 1  8 360 [translate 30 0 0 [cube 2 2 2]]]",
      ),
      engine,
    );
    expect(report).not.toBeNull();
    expect(report!.volume_mm3).toBeCloseTo(8 * 8, 1);
    expect(report!.com_axis_distance_mm).toBeDefined();
    expect(report!.com_axis_distance_mm!.length).toBe(1);
    // Perfect 8-fold symmetry: the CoM sits on the pattern axis.
    expect(report!.com_axis_distance_mm![0]).toBeLessThan(0.05);
    expect(
      report!.warnings.filter((w) => w.includes("off the circular-pattern axis")),
    ).toEqual([]);
  });
});

describe("mutation responses", () => {
  it("attach integrity alongside the changed diff", () => {
    const doc = docFromLoon("[cube 10 10 10]");
    const documentId = registerSession(doc);

    const result = dispatchRegistryTool(
      "create",
      {
        document_id: documentId,
        type: "cylinder",
        params: { radius: 5, height: 20 },
      },
      engine,
    );

    const payload = JSON.parse(result.content[0].text) as {
      changed?: unknown;
      integrity?: {
        volume_mm3: number;
        watertight: boolean;
        parts: number;
        warnings: string[];
      };
    };
    expect(payload.changed).toBeDefined();
    expect(payload.integrity).toBeDefined();
    expect(payload.integrity!.parts).toBe(2);
    // cube 1000 + cylinder π·25·20 ≈ 2570.8 (32-segment tessellation reads
    // the cylinder slightly low).
    expect(payload.integrity!.volume_mm3).toBeGreaterThan(2500);
    expect(payload.integrity!.volume_mm3).toBeLessThan(2600);
    expect(payload.integrity!.watertight).toBe(true);
  });
});
