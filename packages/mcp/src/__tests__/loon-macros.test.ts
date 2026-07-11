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
} from "../tools/loon-macros.js";
import { createCadLoon } from "../tools/loon.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => clearMacrosForTest());

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
  it("stores a macro that passes its smoke call", () => {
    const out = parse(defineLoonTool(FLANGE, engine));
    expect(out.name).toBe("test-flange");
    expect(out.version).toBe(1);
    expect(out.smoke_call).toBe("[test-flange 40 8 5]");
  });

  it("refuses source that does not evaluate — nothing enters the library", () => {
    expect(() =>
      defineLoonTool(
        { ...FLANGE, source: "[let test-flange [fn [od bore t] [cyllinder od t]]]" },
        engine,
      ),
    ).toThrow(/NOT stored/);
    expect(parse(listLoonsTool()).count).toBe(0);
  });

  it("refuses stdlib shadowing and bad names", () => {
    expect(() => defineLoonTool({ ...FLANGE, name: "cube" }, engine)).toThrow(/shadows/);
    expect(() => defineLoonTool({ ...FLANGE, name: "Bad Name" }, engine)).toThrow(/kebab-case/);
  });

  it("redefinition bumps the version", () => {
    defineLoonTool(FLANGE, engine);
    const v2 = parse(defineLoonTool(FLANGE, engine));
    expect(v2.version).toBe(2);
  });
});

describe("call_loon", () => {
  it("instantiates with positional args and mints a session", () => {
    defineLoonTool(FLANGE, engine);
    const out = parse(
      callLoonTool({ name: "test-flange", args: [60, 10, 6], material: "steel" }, engine),
    );
    expect(out.document_id).toBeTruthy();
    expect(out.macro).toBe("test-flange");
    expect(out.document).toContain("steel");
  });

  it("enforces arity with the declared parameter names", () => {
    defineLoonTool(FLANGE, engine);
    expect(() => callLoonTool({ name: "test-flange", args: [60] }, engine)).toThrow(
      /takes 3 args \(od, bore, t\)/,
    );
  });

  it("unknown macro lists what exists", () => {
    defineLoonTool(FLANGE, engine);
    expect(() => callLoonTool({ name: "nope", args: [] }, engine)).toThrow(
      /defined macros: test-flange/,
    );
  });
});

describe("composition via use_loons", () => {
  it("stored macros are callable inside create_cad_loon programs", () => {
    defineLoonTool(FLANGE, engine);
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

  it("missing macro in use_loons errors clearly", () => {
    expect(() =>
      createCadLoon({ source: "[root [cube 1 1 1] \"default\"]", use_loons: ["ghost"] }, engine),
    ).toThrow(/unknown loon macro "ghost"/);
  });
});
