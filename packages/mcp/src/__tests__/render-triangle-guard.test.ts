/**
 * render_view must refuse documents whose tessellation is too dense to
 * render, instead of OOM-killing the process. The drafting renderer emits
 * one SVG element per visible triangle, so a ~380k-triangle document (e.g.
 * a 20×20 sphere pattern) built a >100 MB SVG and took the instance down
 * with it — the agent saw only a 502. The mutation-integrity pass already
 * counts triangles; render_view consults that count up front.
 */

import { describe, it, expect } from "vitest";
import { renderView, rasterize } from "../tools/render.js";
import { openDocument } from "../tools/session.js";
import { recordTriangles } from "../tools/session-core.js";

function openSession(): string {
  const out = openDocument({});
  const parsed = JSON.parse(out.content[0]!.text) as { document_id: string };
  return parsed.document_id;
}

describe("render_view triangle guard", () => {
  it("refuses a session whose last integrity pass exceeded the budget", async () => {
    const id = openSession();
    recordTriangles(id, 384_024);
    const out = await renderView({ document_id: id });
    expect(out.isError).toBe(true);
    const payload = JSON.parse(
      (out.content[0] as { type: "text"; text: string }).text,
    ) as { error: string; hint: string };
    expect(payload.error).toMatch(/render refused/);
    expect(payload.error).toMatch(/384024 triangles/);
    expect(payload.hint).toMatch(/inspect_cad/);
  });

  it("still renders a session under the budget", async () => {
    const id = openSession();
    recordTriangles(id, 61_464);
    const out = await renderView({ document_id: id });
    // Empty doc renders (or degrades to SVG without resvg) — but must not
    // trip the triangle guard.
    const text = out.content.find((c) => c.type === "text") as
      | { type: "text"; text: string }
      | undefined;
    expect(text?.text ?? "").not.toMatch(/render refused/);
  });

  it("rasterize refuses an SVG beyond the raster byte cap", async () => {
    // A structurally valid SVG padded past the cap — must be refused before
    // reaching resvg, not OOM inside it.
    const pad = "<!-- x -->".repeat((64 * 1024 * 1024) / 10 + 1);
    const svg = `<svg xmlns="http://www.w3.org/2000/svg">${pad}</svg>`;
    const out = await rasterize(svg, 200);
    expect(out.png).toBeNull();
    if (out.png === null) {
      expect(out.reason).toMatch(/rasterization refused/);
    }
  });
});
