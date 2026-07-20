/**
 * create_cad_loon session targeting: passing an open session's document_id
 * writes the evaluated document INTO that session instead of minting a new
 * one — keeping the advertised open_document → author workflow on a single
 * id. An unknown id fails loudly instead of silently forking.
 */

import { beforeAll, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { toolDefs } from "../tools/loon.js";
import type { ToolContext } from "../tools/tool-def.js";
import { getSession, registerSession } from "../tools/session-core.js";
import { openInBrowser } from "../tools/share.js";

let engine: Engine;
let ctx: ToolContext;
const createCadLoonDef = toolDefs.find((t) => t.name === "create_cad_loon")!;

beforeAll(async () => {
  engine = await Engine.init();
  ctx = { engine, user: null } as unknown as ToolContext;
});

describe("create_cad_loon document_id targeting", () => {
  it("writes the evaluated document into the given open session", async () => {
    const emptyDoc = engine.evalVcadSource("#[]");
    if (!emptyDoc) throw new Error("engine build lacks loon support");
    const id = registerSession(emptyDoc);

    await createCadLoonDef.handler({ document_id: id, source: "[cube 10 10 10]" }, ctx);

    const doc = getSession(id);
    expect(Object.keys(doc.nodes).length).toBeGreaterThan(0);
  });

  it("advertises document_id in the input schema", () => {
    const props = createCadLoonDef.inputSchema.properties as Record<string, unknown>;
    expect(props.document_id).toBeDefined();
  });

  it("open_in_browser resolves a live session by document_id", () => {
    const doc = engine.evalVcadSource("[cube 5 5 5]");
    if (!doc) throw new Error("engine build lacks loon support");
    const id = registerSession(doc);
    const res = openInBrowser({ document_id: id, name: "cube" });
    expect(res.content[0].text).toContain("https://vcad.io/#/new?");
  });

  it("open_in_browser without document_id or document is a loud error", () => {
    expect(() => openInBrowser({})).toThrow(/document_id.*or.*document/);
  });

  it("fails loudly on an unknown document_id", async () => {
    await expect(
      createCadLoonDef.handler(
        { document_id: "doc_nope_missing", source: "[cube 1 1 1]" },
        ctx,
      ),
    ).rejects.toThrow(/Unknown document_id/);
  });
});
