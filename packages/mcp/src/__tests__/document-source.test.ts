/**
 * Document ⇄ source provenance: a document says what made it, a save writes
 * both forms, a file path opens as a session that is *of* that file, and
 * divergence in either direction is reported rather than silent.
 *
 * The failure this pins is a long design session where several revisions
 * existed only as session documents: `get_document` returned IR, the authored
 * loon was unrecoverable, and a `.loon` edited on disk went on driving nothing
 * until someone remembered to re-evaluate it.
 */

import { mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import { toolDefs as loonToolDefs } from "../tools/loon.js";
import {
  getDocumentTool,
  loadDocument,
  saveDocument,
  openDocument,
} from "../tools/session.js";
import { documents, getSession } from "../tools/session-core.js";
import {
  markSourceDiverged,
  sourceHash,
  sourceStatus,
} from "../tools/source-provenance.js";
import type { SessionStore } from "../session-store.js";
import type { ToolContext } from "../tools/tool-def.js";

const PLATE = `[root [cube 40 20 4] "aluminum"]`;

let engine: Engine;
let ctx: ToolContext;
let dir: string;
const createCadLoon = loonToolDefs.find((t) => t.name === "create_cad_loon")!;

/** The local/stdio store: file-backed save/load under the state root. */
const memoryStore = { scope: "memory" } as unknown as SessionStore;

/** Mint a session and author `source` into it, returning its id. */
async function author(source: string): Promise<string> {
  const opened = JSON.parse(openDocument({}).content[0].text) as {
    document_id: string;
  };
  await createCadLoon.handler(
    { source, document_id: opened.document_id },
    ctx,
  );
  return opened.document_id;
}

beforeAll(async () => {
  engine = await Engine.init();
  ctx = { engine, user: null } as unknown as ToolContext;
  dir = mkdtempSync(join(tmpdir(), "vcad-doc-source-"));
  process.env.VCAD_MCP_STATE_DIR = dir;
});

afterAll(() => {
  delete process.env.VCAD_MCP_STATE_DIR;
  rmSync(dir, { recursive: true, force: true });
});

describe("source on the document", () => {
  it("create_cad_loon records the source it evaluated", async () => {
    const id = await author(PLATE);
    const src = getSession(id).source;
    expect(src?.language).toBe("loon");
    expect(src?.text).toContain("cube 40 20 4");
    expect(src?.hash).toBe(sourceHash(src!.text));
    expect(src?.diverged).toBe(false);
  });

  it("get_document returns the source, not only the IR", async () => {
    const id = await author(PLATE);
    const result = getDocumentTool({ document_id: id });
    const doc = JSON.parse(result.content[0].text) as {
      source?: { text: string };
    };
    expect(doc.source?.text).toContain("cube 40 20 4");
    expect(result.structuredContent?.source_stale).toBe(false);
  });
});

describe("staleness", () => {
  it("an incremental mutation marks the document diverged from its source", async () => {
    const id = await author(PLATE);
    expect(markSourceDiverged(id, "update")).toBe(true);
    // Only the first divergence is "new" — the note is said once.
    expect(markSourceDiverged(id, "delete")).toBe(false);

    const status = sourceStatus(documents.get(id));
    expect(status.source_stale).toBe(true);
    expect(status.reason).toMatch(/update, delete/);
  });

  it("a document with no source is never stale", () => {
    expect(sourceStatus(undefined).source_stale).toBe(false);
    const opened = JSON.parse(openDocument({}).content[0].text) as {
      document_id: string;
    };
    expect(sourceStatus(getSession(opened.document_id)).source_stale).toBe(
      false,
    );
  });

  it("a file edited underneath the session reads as stale", async () => {
    const path = join(dir, "bracket.loon");
    writeFileSync(path, PLATE);
    const loaded = JSON.parse(
      (await loadDocument({ path }, memoryStore, engine)).content[0].text,
    ) as { document_id: string; source_stale: boolean };
    expect(loaded.source_stale).toBe(false);

    writeFileSync(path, `[root [cube 60 20 4] "aluminum"]`);
    const status = sourceStatus(getSession(loaded.document_id));
    expect(status.source_stale).toBe(true);
    expect(status.reason).toMatch(/changed on disk/);
  });
});

describe("load_document from a path", () => {
  it("evaluates a .loon file into a session bound to it", async () => {
    const path = join(dir, "post.loon");
    writeFileSync(path, `[root [cylinder 3 12] "steel"]`);
    const loaded = JSON.parse(
      (await loadDocument({ path }, memoryStore, engine)).content[0].text,
    ) as { document_id: string; path: string };
    const doc = getSession(loaded.document_id);
    expect(doc.roots.length).toBe(1);
    expect(doc.source?.path).toBe(path);
  });

  it("reports an unreadable path instead of minting an empty session", async () => {
    const result = await loadDocument(
      { path: join(dir, "nope.loon") },
      memoryStore,
      engine,
    );
    expect(result.isError).toBe(true);
    expect(result.content[0].text).toMatch(/Cannot read/);
  });
});

describe("save_document", () => {
  it("writes the .loon alongside the .vcad", async () => {
    const id = await author(PLATE);
    const saved = JSON.parse(
      (await saveDocument({ document_id: id, name: "plate" }, memoryStore))
        .content[0].text,
    ) as { path: string; source_path?: string };
    expect(readFileSync(saved.path, "utf8")).toContain('"nodes"');
    expect(saved.source_path).toBeDefined();
    expect(readFileSync(saved.source_path!, "utf8")).toContain("cube 40 20 4");
  });

  it("refuses to write loon that would not reproduce a diverged document", async () => {
    const id = await author(PLATE);
    markSourceDiverged(id, "update");
    const saved = JSON.parse(
      (await saveDocument({ document_id: id, name: "diverged" }, memoryStore))
        .content[0].text,
    ) as { source_path?: string; source_not_written?: string };
    expect(saved.source_path).toBeUndefined();
    expect(saved.source_not_written).toMatch(/would not reproduce/);
  });
});
