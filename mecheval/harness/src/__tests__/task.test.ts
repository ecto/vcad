import { describe, expect, it } from "vitest";
import { resolve } from "node:path";
import { readdir } from "node:fs/promises";
import { loadTask } from "../task.js";

const TASKS_DIR = resolve(__dirname, "..", "..", "..", "tasks");

describe("loadTask", () => {
  it("loads the plate task", async () => {
    const t = await loadTask(resolve(TASKS_DIR, "a1-plate-01.json"));
    expect(t.id).toBe("a1-plate-01");
    expect(t.suite).toBe("A");
    expect(t.checks.length).toBeGreaterThan(0);
  });

  it("loads every seed task", async () => {
    const entries = await readdir(TASKS_DIR);
    const jsons = entries.filter((e) => e.endsWith(".json"));
    expect(jsons.length).toBeGreaterThan(0);
    for (const f of jsons) {
      const t = await loadTask(resolve(TASKS_DIR, f));
      expect(t.id).toBe(f.replace(/\.json$/, ""));
    }
  });
});
