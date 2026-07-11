/**
 * Agent macro library: define → list → call → compose.
 * The contract under test: only macros whose smoke call yields geometry
 * enter the library, and stored macros compose into arbitrary programs
 * exactly like the stdlib.
 */
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { Engine } from "@vcad/engine";
import {
  callLoonTool,
  clearMacrosForTest,
  defineLoonTool,
  listLoonsTool,
  setMacroStoreFactoryForTest,
  type LoonMacro,
} from "../tools/loon-macros.js";
import { createCadLoon } from "../tools/loon.js";
import type { MacroStore } from "../macro-store.js";
import type { AuthUser } from "../oauth.js";

/** In-memory MacroStore double — the durable tier without Supabase. */
class FakeMacroStore implements MacroStore {
  rows = new Map<string, LoonMacro>();
  saves = 0;
  async load(name: string): Promise<LoonMacro | null> {
    return this.rows.get(name) ?? null;
  }
  async list(): Promise<LoonMacro[]> {
    return [...this.rows.values()];
  }
  async save(m: LoonMacro): Promise<void> {
    this.saves++;
    this.rows.set(m.name, m);
  }
}

const USER: AuthUser = { sub: "user-1", email: "cam@example.com" };

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  clearMacrosForTest();
  setMacroStoreFactoryForTest(() => null);
});

const FLANGE = {
  name: "test-flange",
  description: "Disc with a centered bore",
  params: [
    { name: "od", example: 40, unit: "mm" },
    { name: "bore", example: 8, unit: "mm" },
    { name: "t", example: 5, unit: "mm" },
  ],
  source:
    "[let test-flange [fn [od bore t]\n" +
    "  [pipe [cylinder [/ od 2] t]\n" +
    "    [difference [cylinder [/ bore 2] [+ t 2]]]]]]",
};

const parse = (r: { content: Array<{ text: string }> }) =>
  JSON.parse(r.content[0].text);

describe("define_loon", () => {
  it("stores a macro that passes its smoke call", async () => {
    const out = parse(await defineLoonTool(FLANGE, engine));
    expect(out.name).toBe("test-flange");
    expect(out.version).toBe(1);
    expect(out.smoke_call).toBe("[test-flange 40 8 5]");
  });

  it("refuses source that does not evaluate — nothing enters the library", async () => {
    await expect(
      defineLoonTool(
        { ...FLANGE, source: "[let test-flange [fn [od bore t] [cyllinder od t]]]" },
        engine,
      ),
    ).rejects.toThrow(/NOT stored/);
    expect(parse(await listLoonsTool()).count).toBe(0);
  });

  it("refuses stdlib shadowing and bad names", async () => {
    await expect(defineLoonTool({ ...FLANGE, name: "cube" }, engine)).rejects.toThrow(/shadows/);
    await expect(defineLoonTool({ ...FLANGE, name: "Bad Name" }, engine)).rejects.toThrow(
      /kebab-case/,
    );
  });

  it("redefinition bumps the version", async () => {
    await defineLoonTool(FLANGE, engine);
    const v2 = parse(await defineLoonTool(FLANGE, engine));
    expect(v2.version).toBe(2);
  });
});

describe("call_loon", () => {
  it("instantiates with positional args and mints a session", async () => {
    await defineLoonTool(FLANGE, engine);
    const out = parse(
      await callLoonTool(
        { name: "test-flange", args: [60, 10, 6], material: "steel" },
        engine,
      ),
    );
    expect(out.document_id).toBeTruthy();
    expect(out.macro).toBe("test-flange");
    expect(out.document).toContain("steel");
  });

  it("enforces arity with the declared parameter names", async () => {
    await defineLoonTool(FLANGE, engine);
    await expect(
      callLoonTool({ name: "test-flange", args: [60] }, engine),
    ).rejects.toThrow(/takes 3 args \(od, bore, t\)/);
  });

  it("unknown macro lists what exists", async () => {
    await defineLoonTool(FLANGE, engine);
    await expect(callLoonTool({ name: "nope", args: [] }, engine)).rejects.toThrow(
      /defined macros: test-flange/,
    );
  });
});

describe("composition via use_loons", () => {
  it("stored macros are callable inside create_cad_loon programs", async () => {
    await defineLoonTool(FLANGE, engine);
    const result = createCadLoon(
      {
        source:
          "[root [union [translate 50 0 0 [test-flange 30 6 4]] [test-flange 40 8 5]] \"aluminum\"]",
        use_loons: ["test-flange"],
        format: "json",
      },
      engine,
    );
    const doc = JSON.parse(result.content[0].text);
    expect(doc.roots?.length).toBeGreaterThan(0);
  });

  it("STATELESS: inline `loons` work with an empty registry (cold start)", async () => {
    // No define_loon — simulates a fresh serverless instance. The macro
    // record travels by value, as returned by define_loon.
    const result = createCadLoon(
      {
        source: "[root [test-flange 40 8 5] \"aluminum\"]",
        loons: [{ name: FLANGE.name, source: FLANGE.source }],
        format: "json",
      },
      engine,
    );
    const doc = JSON.parse(result.content[0].text);
    expect(doc.roots?.length).toBeGreaterThan(0);
  });

  it("STATELESS: call_loon with an inline macro, params optional", async () => {
    const out = parse(
      await callLoonTool(
        {
          name: "test-flange",
          args: [60, 10, 6],
          macro: { name: FLANGE.name, source: FLANGE.source },
        },
        engine,
      ),
    );
    expect(out.document_id).toBeTruthy();
  });

  it("define_loon returns the portable macro record", async () => {
    const out = parse(await defineLoonTool(FLANGE, engine));
    expect(out.macro).toEqual({
      name: FLANGE.name,
      source: FLANGE.source,
      params: FLANGE.params,
    });
  });

  it("missing macro in use_loons errors clearly", async () => {
    expect(() =>
      createCadLoon({ source: "[root [cube 1 1 1] \"default\"]", use_loons: ["ghost"] }, engine),
    ).toThrow(/unknown loon macro "ghost"/);
  });
});

describe("hosted durability (MacroStore)", () => {
  it("define_loon saves to the durable store for a signed-in user", async () => {
    const store = new FakeMacroStore();
    setMacroStoreFactoryForTest((u) => (u ? store : null));
    await defineLoonTool(FLANGE, engine, USER);
    expect(store.saves).toBe(1);
    expect(store.rows.get("test-flange")?.version).toBe(1);
  });

  it("cold start: call_loon hydrates the macro from the store on miss", async () => {
    const store = new FakeMacroStore();
    store.rows.set("test-flange", { ...FLANGE, version: 3 });
    setMacroStoreFactoryForTest((u) => (u ? store : null));
    // Registry is empty (fresh instance) — the durable row makes the call work.
    const out = parse(
      await callLoonTool({ name: "test-flange", args: [60, 10, 6] }, engine, USER),
    );
    expect(out.document_id).toBeTruthy();
    expect(out.version).toBe(3);
  });

  it("cold start: redefinition continues the cloud version sequence", async () => {
    const store = new FakeMacroStore();
    store.rows.set("test-flange", { ...FLANGE, version: 4 });
    setMacroStoreFactoryForTest((u) => (u ? store : null));
    const out = parse(await defineLoonTool(FLANGE, engine, USER));
    expect(out.version).toBe(5);
  });

  it("list_loons merges the cloud library; anonymous users stay warm-only", async () => {
    const store = new FakeMacroStore();
    store.rows.set("test-flange", { ...FLANGE, version: 2 });
    setMacroStoreFactoryForTest((u) => (u ? store : null));
    expect(parse(await listLoonsTool(null)).count).toBe(0);
    const signedIn = parse(await listLoonsTool(USER));
    expect(signedIn.count).toBe(1);
    expect(signedIn.macros[0].version).toBe(2);
  });
});
