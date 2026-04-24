import { describe, expect, it } from "vitest";
import type { Document } from "@vcad/ir";
import {
  EvalError,
  ParseError,
  evalAst,
  evaluate,
  freeVars,
  parse,
  parseBindingKey,
  resolveDocument,
  resolveParameters,
} from "../expressions.js";

describe("expression parser (TS, matches tang-expr)", () => {
  const env = { x: 3, y: 4, wheelbase: 1000 };

  it.each([
    ["1+2", 3],
    ["1 - 2", -1],
    ["2 * 3", 6],
    ["10 / 4", 2.5],
    ["7 % 3", 1],
    ["1 + 2 * 3", 7],
    ["(1 + 2) * 3", 9],
    ["2 ^ 3 ^ 2", 512], // right-assoc
    ["-2 ^ 2", -4], // unary minus is looser than ^
    ["--3", 3],
    ["+-3", -3],
    ["1e3", 1000],
    ["2.5e-2", 0.025],
    ["x + y", 7],
    ["wheelbase * 0.5", 500],
    ["pow(2, 10)", 1024],
    ["min(3, 7)", 3],
    ["max(3, 7)", 7],
    ["round(2.7)", 3],
    ["abs(-5)", 5],
  ])("%s → %f", (src, expected) => {
    expect(evaluate(src, env)).toBeCloseTo(expected, 12);
  });

  it("constants pi/tau/e", () => {
    expect(evaluate("pi", {})).toBeCloseTo(Math.PI, 12);
    expect(evaluate("tau", {})).toBeCloseTo(Math.PI * 2, 12);
    expect(evaluate("e", {})).toBeCloseTo(Math.E, 12);
  });

  it("deg/rad helpers", () => {
    expect(evaluate("deg(pi)", {})).toBeCloseTo(180, 12);
    expect(evaluate("rad(180)", {})).toBeCloseTo(Math.PI, 12);
  });

  it("sqrt of pythagorean triple", () => {
    expect(evaluate("sqrt(pow(3, 2) + pow(4, 2))", {})).toBeCloseTo(5, 12);
  });

  it("undefined variable surfaces EvalError", () => {
    expect(() => evaluate("missing + 1", {})).toThrow(EvalError);
  });

  it("unknown function surfaces EvalError", () => {
    expect(() => evaluate("bogus(1)", {})).toThrow(EvalError);
  });

  it("arity mismatch surfaces EvalError", () => {
    expect(() => evaluate("min(1)", {})).toThrow(EvalError);
  });

  it("trailing garbage is a ParseError", () => {
    expect(() => parse("1 + 2 3")).toThrow(ParseError);
  });

  it("invalid binary operator has offset", () => {
    try {
      parse("1 + * 2");
      throw new Error("expected throw");
    } catch (e) {
      expect(e).toBeInstanceOf(ParseError);
      expect((e as ParseError).offset).toBeGreaterThanOrEqual(4);
    }
  });

  it("division by zero", () => {
    expect(() => evaluate("1/0", {})).toThrow(EvalError);
  });

  it("free vars skip constants", () => {
    const ast = parse("pi * r ^ 2 + x");
    expect(freeVars(ast).sort()).toEqual(["r", "x"]);
  });

  it("evalAst works on pre-parsed AST", () => {
    const ast = parse("a + b * 2");
    expect(evalAst(ast, { a: 1, b: 5 })).toBe(11);
  });
});

describe("parameter resolution", () => {
  it("evaluates literals and chains", () => {
    const env = resolveParameters({
      a: { value: 5 },
      b: { value: "a + 10" },
      c: { value: "b * 2" },
    });
    expect(env).toEqual({ a: 5, b: 15, c: 30 });
  });

  it("detects cycles", () => {
    expect(() =>
      resolveParameters({
        a: { value: "b + 1" },
        b: { value: "a + 1" },
      }),
    ).toThrow(/cycle/);
  });

  it("returns empty for undefined params", () => {
    expect(resolveParameters(undefined)).toEqual({});
  });
});

describe("resolveDocument", () => {
  function docWithCubeAndTranslate(): Document {
    return {
      version: "0.1",
      nodes: {
        "1": {
          id: 1,
          name: "cube",
          op: { type: "Cube", size: { x: 0, y: 0, z: 0 } },
        },
        "2": {
          id: 2,
          name: "shift",
          op: {
            type: "Translate",
            child: 1,
            offset: { x: 0, y: 0, z: 0 },
          },
        },
      },
      materials: {},
      part_materials: {},
      roots: [],
    };
  }

  it("applies bindings derived from parameters", () => {
    const doc = docWithCubeAndTranslate();
    doc.parameters = {
      w: { value: 50 },
    };
    doc.bindings = {
      "1:size.x": "w * 2",
      "1:size.z": 17.5,
      "2:offset.z": "w",
    };
    const { doc: patched, env } = resolveDocument(doc);
    expect(env.w).toBe(50);
    const cube = patched.nodes["1"].op as {
      type: "Cube";
      size: { x: number; y: number; z: number };
    };
    expect(cube.size.x).toBe(100);
    expect(cube.size.z).toBe(17.5);
    const tr = patched.nodes["2"].op as {
      type: "Translate";
      offset: { x: number; y: number; z: number };
    };
    expect(tr.offset.z).toBe(50);
    // Source must be unmodified.
    const origCube = doc.nodes["1"].op as { size: { x: number } };
    expect(origCube.size.x).toBe(0);
  });

  it("is a no-op when no params and no bindings", () => {
    const doc = docWithCubeAndTranslate();
    const { doc: patched, env } = resolveDocument(doc);
    expect(env).toEqual({});
    // Returns same object (shallow), no deep-clone overhead.
    expect(patched).toBe(doc);
  });

  it("parseBindingKey parses node:path", () => {
    expect(parseBindingKey("42:size.x")).toEqual({ nodeId: "42", fieldPath: "size.x" });
    expect(parseBindingKey("invalid")).toBeNull();
  });

  it("rounds integer-like fields (segments, count)", () => {
    const doc: Document = {
      version: "0.1",
      nodes: {
        "1": {
          id: 1,
          name: null,
          op: { type: "Cylinder", radius: 5, height: 10, segments: 0 },
        },
      },
      materials: {},
      part_materials: {},
      roots: [],
      bindings: { "1:segments": "16.7" },
    };
    const { doc: patched } = resolveDocument(doc);
    const cyl = patched.nodes["1"].op as { segments: number };
    expect(cyl.segments).toBe(17);
  });
});
