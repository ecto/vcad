/**
 * Session loss must be SELF-ANNOUNCING.
 *
 * The failure this pins (observed 2026-07-25): a non-durable server restarted
 * mid-session, every live document_id silently died, and the first symptom was
 * an unrelated call failing many turns later with an error indistinguishable
 * from a typo'd id — while every mounted preview widget went dark at once and
 * looked like a broken renderer. Three defenses, tested here:
 *
 *  1. ids carry the minting process's boot token, so "lost to a restart" is
 *     mechanically separable from "no such id";
 *  2. local runs persist to disk, so the restart doesn't destroy the work;
 *  3. a mint under a non-durable store says so, up front.
 */
import { describe, it, expect, beforeEach, afterEach, beforeAll } from "vitest";
import { mkdtempSync, mkdirSync, readdirSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { Document } from "@vcad/ir";
import { createServer } from "../server.js";
import { FileSessionStore, sessionDir, useFileSessionStore } from "../session-store.js";
import {
  documents,
  nextSessionId,
  getSession,
  hydrateSession,
  persistSession,
  dropSession,
  currentBootToken,
  sessionIdBootToken,
  isForeignSessionId,
  unknownSessionMessage,
  durabilityWarning,
} from "../tools/session.js";

function makeDoc(): Document {
  return { nodes: {}, roots: [] } as unknown as Document;
}

/** A session id minted by a DIFFERENT process — i.e. one that survived a
 *  restart in an agent's context while its contents did not. */
function foreignId(): string {
  const ours = currentBootToken();
  const other = ours[0] === "z" ? `y${ours.slice(1)}` : `z${ours.slice(1)}`;
  return `doc_7_${other}AAAAAAAAAAAA`;
}

const ENV_KEYS = ["VCAD_MCP_DISK_SESSIONS", "SUPABASE_URL", "SUPABASE_SERVICE_ROLE_KEY"] as const;

describe("boot-generation tagging", () => {
  it("mints ids carrying THIS process's boot token", () => {
    const id = nextSessionId();
    expect(sessionIdBootToken(id)).toBe(currentBootToken());
    expect(isForeignSessionId(id)).toBe(false);
  });

  it("recognizes an id minted by an earlier process as foreign", () => {
    expect(isForeignSessionId(foreignId())).toBe(true);
  });

  it("treats an untagged / hand-written id as not-foreign (no false alarm)", () => {
    // Better to fall back to the generic message than to tell someone their
    // typo was a server restart.
    expect(sessionIdBootToken("not-a-session-id")).toBeNull();
    expect(isForeignSessionId("not-a-session-id")).toBe(false);
  });

  it("ids stay unguessable — the token is a prefix, not a replacement", () => {
    const a = nextSessionId();
    const b = nextSessionId();
    expect(a).not.toBe(b);
    // Boot token + at least the 12 base64url chars of the 9 random bytes.
    // NB: base64url uses "_", so the suffix can itself contain underscores —
    // splitting on "_" and taking [2] truncates it ~22% of the time.
    const suffix = (id: string) => id.replace(/^doc_\d+_/, "");
    expect(suffix(a)).not.toBe(a);
    for (const id of [a, b]) expect(suffix(id).length).toBeGreaterThanOrEqual(16);
  });
});

describe("unknownSessionMessage distinguishes the two causes", () => {
  let saved: Record<string, string | undefined>;
  beforeEach(() => {
    saved = {};
    for (const k of ENV_KEYS) saved[k] = process.env[k];
  });
  afterEach(() => {
    for (const k of ENV_KEYS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
  });

  it("names the RESTART (not a typo) for a foreign id on a non-durable store", () => {
    process.env.VCAD_MCP_DISK_SESSIONS = "0";
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    const msg = unknownSessionMessage(foreignId());
    expect(msg).toMatch(/SESSION LOST TO A SERVER RESTART/);
    expect(msg).toMatch(/not a typo/i);
    // The remediation must be actionable: re-author, and how to check.
    expect(msg).toMatch(/re-run the authoring calls/i);
    expect(msg).toMatch(/server_info/);
    // The pinned prefix every caller greps for is preserved.
    expect(msg).toMatch(/^Unknown document_id/);
  });

  it("does NOT cry restart-loss when the store is durable", () => {
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    const msg = unknownSessionMessage(foreignId());
    expect(msg).not.toMatch(/SESSION LOST/);
    expect(msg).toMatch(/durable session store/);
  });

  it("reads as a typo for an id this process could have minted", () => {
    const msg = unknownSessionMessage(nextSessionId());
    expect(msg).not.toMatch(/SESSION LOST/);
    expect(msg).toMatch(/typo/);
  });

  it("getSession raises the specialized message", () => {
    process.env.VCAD_MCP_DISK_SESSIONS = "0";
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(() => getSession(foreignId())).toThrow(/SESSION LOST TO A SERVER RESTART/);
  });
});

describe("FileSessionStore survives the restart", () => {
  // The session dir is NESTED inside a sandbox so the traversal test can assert
  // on the sandbox's contents. Probing an absolute host path (e.g.
  // join(dir, "..", "..", "etc", "passwd")) is not a test of this code: on a
  // shallow tmpdir like Linux CI's /tmp it resolves to the REAL /etc/passwd and
  // fails no matter how airtight the guard is.
  let sandbox: string;
  let dir: string;
  beforeEach(() => {
    sandbox = mkdtempSync(join(tmpdir(), "vcad-sessions-"));
    dir = join(sandbox, "sessions");
    mkdirSync(dir);
    documents.clear();
  });
  afterEach(() => {
    rmSync(sandbox, { recursive: true, force: true });
    documents.clear();
  });

  it("round-trips a document through disk", async () => {
    const store = new FileSessionStore(dir);
    const id = nextSessionId();
    await store.save(id, makeDoc());
    expect(await store.load(id)).toEqual(makeDoc());
    await store.drop(id);
    expect(await store.load(id)).toBeNull();
  });

  it("REGRESSION: a restart no longer loses the work", async () => {
    const store = new FileSessionStore(dir);
    const id = nextSessionId();
    documents.set(id, makeDoc());
    await persistSession(store, id);

    // ── The restart: every warm cache is gone.
    documents.clear();
    expect(documents.has(id)).toBe(false);

    // The dispatch layer's hydrate-on-miss brings it back instead of throwing.
    expect(await hydrateSession(store, id)).toBe(true);
    expect(() => getSession(id)).not.toThrow();
  });

  it("close_document forgets the file too (no orphaned snapshots)", async () => {
    const store = new FileSessionStore(dir);
    const id = nextSessionId();
    documents.set(id, makeDoc());
    await persistSession(store, id);
    expect(existsSync(join(dir, `${id}.json`))).toBe(true);
    await dropSession(store, id);
    expect(existsSync(join(dir, `${id}.json`))).toBe(false);
  });

  it("refuses an id that could escape the session directory", async () => {
    const store = new FileSessionStore(dir);
    for (const id of ["../../etc/passwd", "../escaped", "..", "/abs/path"]) {
      await store.save(id, makeDoc());
      expect(await store.load(id)).toBeNull();
    }
    // Nothing escaped: the sandbox still holds only the session dir, and the
    // session dir itself is untouched. "../escaped" would have landed right
    // here if the guard let it through.
    expect(readdirSync(sandbox)).toEqual(["sessions"]);
    expect(readdirSync(dir)).toEqual([]);
  });

  it("a corrupt snapshot is a miss, not a crash", async () => {
    const store = new FileSessionStore(dir);
    const { writeFileSync } = await import("node:fs");
    writeFileSync(join(dir, "doc_1_abcd.json"), "{ truncated", "utf8");
    expect(await store.load("doc_1_abcd")).toBeNull();
  });
});

describe("end-to-end: a local restart no longer loses the document", () => {
  let engine: Engine;
  let dir: string;
  let saved: Record<string, string | undefined>;

  beforeAll(async () => {
    engine = await Engine.init();
  });

  beforeEach(() => {
    saved = {};
    for (const k of ENV_KEYS) saved[k] = process.env[k];
    for (const k of ENV_KEYS) delete process.env[k];
    dir = mkdtempSync(join(tmpdir(), "vcad-sessions-e2e-"));
    process.env.VCAD_MCP_SESSION_DIR = dir;
    documents.clear();
  });
  afterEach(() => {
    for (const k of ENV_KEYS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
    delete process.env.VCAD_MCP_SESSION_DIR;
    rmSync(dir, { recursive: true, force: true });
    documents.clear();
  });

  async function connect() {
    const server = await createServer(engine, { user: null });
    const [ct, st] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "t", version: "0.0.0" }, { capabilities: {} });
    await Promise.all([client.connect(ct), server.connect(st)]);
    return { client, server };
  }

  const text = (r: unknown): string =>
    (r as { content: Array<{ text: string }> }).content[0].text;

  const blocksOf = (r: unknown): Array<{ text?: string }> =>
    (r as { content?: Array<{ text?: string }> }).content ?? [];

  /** create_cad_loon leads with a VCode text block, so scan for the JSON one. */
  const docIdOf = (r: unknown): string => {
    for (const b of blocksOf(r)) {
      try {
        const parsed = JSON.parse(b.text ?? "") as { document_id?: string };
        if (parsed.document_id) return parsed.document_id;
      } catch {
        /* not the JSON block */
      }
    }
    throw new Error(`no document_id in result: ${JSON.stringify(blocksOf(r))}`);
  };

  // The 2026-07-25 incident, start to finish: author a part, restart the
  // server under it, and use the SAME document_id afterwards.
  it("a document authored on instance A still resolves after a restart", async () => {
    const a = await connect();
    const created = await a.client.callTool({
      name: "create_cad_loon",
      arguments: { source: "[root [cube 10 10 10] \"part\"]" },
    });
    expect((created as { isError?: boolean }).isError ?? false).toBe(false);
    const id = docIdOf(created);
    await a.client.close();
    await a.server.close();

    // ── The restart: a brand-new process's empty cache.
    documents.clear();
    const b = await connect();

    const got = await b.client.callTool({
      name: "inspect_cad",
      arguments: { document_id: id },
    });
    expect((got as { isError?: boolean }).isError ?? false).toBe(false);
    // The actual geometry came back, not just a live handle: 10³ mm.
    expect(JSON.parse(text(got)).volume_mm3).toBeCloseTo(1000, 6);

    await b.client.close();
    await b.server.close();
  });

  it("with disk sessions off, the mint warns that the work is unrecoverable", async () => {
    process.env.VCAD_MCP_DISK_SESSIONS = "0";
    const a = await connect();
    const created = await a.client.callTool({
      name: "create_cad_loon",
      arguments: { source: "[root [cube 10 10 10] \"part\"]" },
    });
    // The agent is told AT MINT TIME to keep the authoring source — the only
    // moment it can still act on it.
    expect(
      blocksOf(created).some((b) => /NON-DURABLE SESSION/.test(b.text ?? "")),
    ).toBe(true);
    await a.client.close();
    await a.server.close();
  });

  // A true restart can't be simulated in-process (the boot token is per
  // process), so drive the dispatch path with an id bearing a FOREIGN token —
  // exactly what an agent holds after its server restarted underneath it.
  it("dispatch surfaces the restart diagnosis for an id from a dead process", async () => {
    process.env.VCAD_MCP_DISK_SESSIONS = "0";
    const b = await connect();
    const got = (await b.client.callTool({
      name: "inspect_cad",
      arguments: { document_id: foreignId() },
    })) as { isError?: boolean; content: Array<{ text: string }> };

    expect(got.isError).toBe(true);
    // Not a mystery, and not mistakable for a typo.
    expect(got.content[0].text).toMatch(/SESSION LOST TO A SERVER RESTART/);
    expect(got.content[0].text).toMatch(/re-run the authoring calls/i);

    await b.client.close();
    await b.server.close();
  });
});

describe("non-durable storage announces itself at mint time", () => {
  let saved: Record<string, string | undefined>;
  beforeEach(() => {
    saved = {};
    for (const k of ENV_KEYS) saved[k] = process.env[k];
  });
  afterEach(() => {
    for (const k of ENV_KEYS) {
      if (saved[k] === undefined) delete process.env[k];
      else process.env[k] = saved[k];
    }
  });

  it("warns, with the remedy, when sessions are memory-only", () => {
    process.env.VCAD_MCP_DISK_SESSIONS = "0";
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    const w = durabilityWarning();
    expect(w).toMatch(/NON-DURABLE SESSION/);
    // Tell the agent to keep the source — the loss is otherwise unrecoverable.
    expect(w).toMatch(/keep the authoring source/i);
    expect(w).toMatch(/checkpoint_document/);
  });

  it("stays quiet when the store is durable", () => {
    process.env.SUPABASE_URL = "https://supa.test";
    process.env.SUPABASE_SERVICE_ROLE_KEY = "svc";
    expect(durabilityWarning()).toBeUndefined();
  });

  it("local runs are durable by default, under a real directory", () => {
    delete process.env.VCAD_MCP_DISK_SESSIONS;
    delete process.env.SUPABASE_SERVICE_ROLE_KEY;
    expect(useFileSessionStore()).toBe(true);
    expect(sessionDir()).toMatch(/mcp-sessions$/);
    expect(durabilityWarning()).toBeUndefined();
  });
});
