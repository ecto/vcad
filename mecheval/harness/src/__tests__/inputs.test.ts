import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { partitionInputs, resolveAgentInputs } from "../inputs.js";
import type { Task } from "../task.js";

describe("partitionInputs", () => {
  it("strips agent_visible: false structured inputs", () => {
    const { visible, private: priv } = partitionInputs([
      { kind: "reference_image", agent_visible: true, path: "a.png" },
      { kind: "host_geometry", agent_visible: false, path: "host.vcad" },
      { kind: "known_dimensions", agent_visible: true, text: "10mm" },
    ]);
    expect(visible).toHaveLength(2);
    expect(priv).toHaveLength(1);
    expect((priv[0] as { kind: string }).kind).toBe("host_geometry");
  });

  it("treats bare path inputs as visible", () => {
    const { visible, private: priv } = partitionInputs(["legacy/starter.vcad"]);
    expect(visible).toEqual(["legacy/starter.vcad"]);
    expect(priv).toEqual([]);
  });

  it("handles missing inputs", () => {
    const { visible, private: priv } = partitionInputs(undefined);
    expect(visible).toEqual([]);
    expect(priv).toEqual([]);
  });
});

describe("resolveAgentInputs", () => {
  let tmp: string;

  beforeAll(async () => {
    tmp = await mkdtemp(join(tmpdir(), "mecheval-inputs-"));
    await mkdir(join(tmp, "assets"), { recursive: true });
    // Minimal 1x1 PNG (red pixel) — bytes from a known reference.
    const png = Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==",
      "base64",
    );
    await writeFile(join(tmp, "assets/front.png"), png);
  });

  afterAll(async () => {
    await rm(tmp, { recursive: true, force: true });
  });

  it("base64-encodes images and strips private inputs", async () => {
    const task: Task = {
      id: "t",
      suite: "F",
      tier: "F1",
      title: "t",
      prompt: "p",
      checks: [],
      inputs: [
        {
          kind: "reference_image",
          agent_visible: true,
          path: "assets/front.png",
          view: "front",
          image_kind: "photo",
        },
        { kind: "host_geometry", agent_visible: false, path: "host.vcad" },
        { kind: "known_dimensions", agent_visible: true, text: "10mm" },
      ],
    };
    const out = await resolveAgentInputs(task, tmp);
    // host_geometry must be filtered out.
    expect(out).toHaveLength(2);
    const img = out.find((a) => a.kind === "reference_image");
    expect(img).toBeDefined();
    if (img && img.kind === "reference_image") {
      expect(img.mime).toBe("image/png");
      expect(img.base64.length).toBeGreaterThan(0);
      expect(img.meta.view).toBe("front");
    }
    const dim = out.find((a) => a.kind === "known_dimensions");
    expect(dim).toBeDefined();
    if (dim && dim.kind === "known_dimensions") {
      expect(dim.text).toBe("10mm");
    }
  });

  it("propagates bare path inputs as 'other' kind", async () => {
    const task: Task = {
      id: "legacy",
      suite: "A",
      tier: "A1",
      title: "t",
      prompt: "p",
      checks: [],
      inputs: ["starter.vcad"],
    };
    const out = await resolveAgentInputs(task, tmp);
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("other");
  });
});
