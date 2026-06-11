/**
 * Tests for the verify-and-iterate loop tools: render_view (agent eyes)
 * and verify_part / list_eval_tasks (self-grading oracle).
 *
 * verify_part shells out to the mecheval-grade binary; those cases are
 * skipped when it hasn't been built (`cargo build -p mecheval-grader
 * --bin mecheval-grade`).
 */

import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { renderView } from "../tools/render.js";
import { verifyPart, listEvalTasks } from "../tools/verify.js";
import { openDocument, documents } from "../tools/session.js";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");
const graderAvailable = ["release", "debug"].some((profile) =>
  existsSync(join(repoRoot, "target", profile, "mecheval-grade")),
);

/** Minimal Document with one 10mm cube part. */
function makeCubeDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "test_cube",
        op: { type: "Cube", size: { x: 10, y: 10, z: 10 } },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  };
}

/** The a1-block-01 reference solution: 60×40×20 block centered in X/Y,
 *  bottom face on the XY plane. */
function makeBlockDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": {
        id: 1,
        name: "Block",
        op: { type: "Cube", size: { x: 60, y: 40, z: 20 } },
      },
      "2": {
        id: 2,
        name: "Centered",
        op: {
          type: "Translate",
          child: 1,
          offset: { x: -30, y: -20, z: 0 },
        },
      },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 2, material: "default" }],
  };
}

function openWith(doc: Document): string {
  const open = openDocument({ initial: doc });
  return JSON.parse(open.content[0].text).document_id as string;
}

beforeAll(async () => {
  await Engine.init();
});

beforeEach(() => {
  documents.clear();
});

describe("render_view", () => {
  it("returns a PNG image block for a cube document", async () => {
    const documentId = openWith(makeCubeDoc());
    const out = await renderView({ document_id: documentId });
    expect(out.isError).toBeFalsy();

    const image = out.content.find((c) => c.type === "image") as
      | { type: "image"; data: string; mimeType: string }
      | undefined;
    expect(image, "expected an image block (is @resvg/resvg-js installed?)").toBeDefined();
    expect(image!.mimeType).toBe("image/png");
    const png = Buffer.from(image!.data, "base64");
    // PNG magic bytes — proves real rasterization, not an empty buffer.
    expect(png.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
    expect(png.length).toBeGreaterThan(500);

    const meta = out.content.find((c) => c.type === "text") as
      | { type: "text"; text: string }
      | undefined;
    expect(meta).toBeDefined();
    expect(JSON.parse(meta!.text).view).toBe("isometric");
  });

  it("respects width_px clamping", async () => {
    const documentId = openWith(makeCubeDoc());
    const out = await renderView({ document_id: documentId, width_px: 128 });
    const meta = out.content.find((c) => c.type === "text") as {
      type: "text";
      text: string;
    };
    expect(JSON.parse(meta.text).width_px).toBe(128);
  });

  it("fails loudly on an empty document", async () => {
    const documentId = openWith({
      version: "0.1",
      nodes: {},
      materials: {},
      part_materials: {},
      roots: [],
    } as unknown as Document);
    const out = await renderView({ document_id: documentId });
    expect(out.isError).toBe(true);
    const text = (out.content[0] as { text: string }).text;
    expect(text).toContain("render failed");
  });

  it("throws a helpful error for unknown document ids", async () => {
    await expect(renderView({ document_id: "doc_missing" })).rejects.toThrow(
      /Unknown document_id/,
    );
  });
});

describe("list_eval_tasks", () => {
  it("lists tasks with ids, prompts, and check counts", () => {
    const out = listEvalTasks({});
    expect(out.isError).toBeFalsy();
    const parsed = JSON.parse(out.content[0].text);
    expect(parsed.count).toBeGreaterThan(40);
    const block = parsed.tasks.find(
      (t: { id: string }) => t.id === "a1-block-01",
    );
    expect(block).toBeDefined();
    expect(block.prompt).toContain("60mm");
    expect(block.checks).toBeGreaterThan(0);
  });

  it("filters by suite", () => {
    const out = listEvalTasks({ suite: "A" });
    const parsed = JSON.parse(out.content[0].text);
    expect(parsed.count).toBeGreaterThan(0);
    for (const task of parsed.tasks) {
      expect(String(task.suite).toUpperCase()).toBe("A");
    }
  });
});

describe("verify_part", () => {
  it("rejects path-traversal task ids", () => {
    const documentId = openWith(makeCubeDoc());
    const out = verifyPart({
      document_id: documentId,
      task_id: "../runs/evil",
    });
    expect(out.isError).toBe(true);
    expect(out.content[0].text).toContain("invalid task_id");
  });

  it("errors helpfully on unknown task ids", () => {
    const documentId = openWith(makeCubeDoc());
    const out = verifyPart({
      document_id: documentId,
      task_id: "no-such-task-9999",
    });
    expect(out.isError).toBe(true);
    expect(out.content[0].text).toContain("list_eval_tasks");
  });

  it.skipIf(!graderAvailable)(
    "passes a correct a1-block-01 solution",
    () => {
      const documentId = openWith(makeBlockDoc());
      const out = verifyPart({
        document_id: documentId,
        task_id: "a1-block-01",
      });
      expect(out.isError).toBeFalsy();
      const parsed = JSON.parse(out.content[0].text);
      expect(parsed.summary.passed).toBe(true);
      expect(parsed.checks.length).toBeGreaterThan(0);
      for (const check of parsed.checks) {
        expect(check.result).toBe("pass");
      }
    },
    120_000,
  );

  it.skipIf(!graderAvailable)(
    "fails a wrong part with per-check feedback",
    () => {
      const documentId = openWith(makeCubeDoc()); // 10mm cube ≠ 60×40×20 block
      const out = verifyPart({
        document_id: documentId,
        task_id: "a1-block-01",
      });
      expect(out.isError).toBeFalsy();
      const parsed = JSON.parse(out.content[0].text);
      expect(parsed.summary.passed).toBe(false);
      // The bbox check must fail and carry actionable details.
      const bbox = parsed.checks.find(
        (c: { type: string }) => c.type === "bbox",
      );
      expect(bbox).toBeDefined();
      expect(bbox.result).not.toBe("pass");
    },
    120_000,
  );
});
