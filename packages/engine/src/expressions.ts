/**
 * Parametric expression parser and resolver.
 *
 * Mirrors the grammar and semantics of `tang-expr::parser` on the Rust
 * side. Both evaluators consume the same `.vcad` document shape and
 * MUST agree bit-for-bit on shared fixtures (see
 * `__tests__/expressions.test.ts`).
 *
 * Grammar (Pratt-style precedence climbing):
 *
 * ```
 * expr    := term (('+' | '-') term)*
 * term    := unary (('*' | '/' | '%') unary)*
 * unary   := ('-' | '+') unary | power
 * power   := atom ('^' unary)?                   // right-assoc, tighter than unary
 * atom    := NUMBER | IDENT ('(' args? ')')? | '(' expr ')'
 * ```
 *
 * Supported functions: sin, cos, tan, asin, acos, atan, atan2, sqrt,
 * abs, floor, ceil, round, ln, log (=ln), log2, exp, pow, min, max,
 * deg, rad. Constants: pi, tau, e.
 */

import type { Bindings, Document, Expr, Parameter } from "@vcad/ir";

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

export type Ast =
  | { kind: "num"; value: number }
  | { kind: "ident"; name: string }
  | { kind: "binary"; op: BinOp; lhs: Ast; rhs: Ast }
  | { kind: "unary"; op: UnOp; arg: Ast }
  | { kind: "call"; name: string; args: Ast[] };

export type BinOp = "+" | "-" | "*" | "/" | "%" | "^";
export type UnOp = "+" | "-";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export class ParseError extends Error {
  readonly offset: number;
  constructor(message: string, offset: number) {
    super(`parse error at offset ${offset}: ${message}`);
    this.name = "ParseError";
    this.offset = offset;
  }
}

export class EvalError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "EvalError";
  }
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

type Tok =
  | { kind: "num"; value: number; offset: number }
  | { kind: "ident"; value: string; offset: number }
  | { kind: "sym"; value: string; offset: number }
  | { kind: "end"; offset: number };

function isDigit(c: string): boolean {
  return c >= "0" && c <= "9";
}
function isAlpha(c: string): boolean {
  return (c >= "A" && c <= "Z") || (c >= "a" && c <= "z") || c === "_";
}
function isAlnum(c: string): boolean {
  return isAlpha(c) || isDigit(c);
}

function lex(src: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === " " || c === "\t" || c === "\n" || c === "\r") {
      i++;
      continue;
    }
    if (isDigit(c) || (c === "." && i + 1 < src.length && isDigit(src[i + 1]))) {
      const start = i;
      let hasDot = false;
      let hasExp = false;
      while (i < src.length) {
        const ch = src[i];
        if (isDigit(ch)) {
          i++;
        } else if (ch === "." && !hasDot && !hasExp) {
          hasDot = true;
          i++;
        } else if ((ch === "e" || ch === "E") && !hasExp) {
          hasExp = true;
          i++;
          if (i < src.length && (src[i] === "+" || src[i] === "-")) i++;
        } else {
          break;
        }
      }
      const lit = src.slice(start, i);
      const n = Number(lit);
      if (!Number.isFinite(n)) throw new ParseError(`invalid number '${lit}'`, start);
      out.push({ kind: "num", value: n, offset: start });
      continue;
    }
    if (isAlpha(c)) {
      const start = i;
      while (i < src.length && isAlnum(src[i])) i++;
      out.push({ kind: "ident", value: src.slice(start, i), offset: start });
      continue;
    }
    if ("+-*/%^(),".includes(c)) {
      out.push({ kind: "sym", value: c, offset: i });
      i++;
      continue;
    }
    throw new ParseError(`unexpected character '${c}'`, i);
  }
  out.push({ kind: "end", offset: src.length });
  return out;
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

export function parse(src: string): Ast {
  const toks = lex(src);
  let pos = 0;
  const peek = () => toks[pos];
  const eat = (): Tok => toks[pos++];
  const eatSym = (want: string, what: string) => {
    const t = peek();
    if (t.kind !== "sym" || t.value !== want)
      throw new ParseError(`expected ${what}`, t.offset);
    pos++;
  };

  const parseExpr = (): Ast => {
    let lhs = parseTerm();
    while (true) {
      const t = peek();
      if (t.kind !== "sym" || (t.value !== "+" && t.value !== "-")) break;
      pos++;
      const rhs = parseTerm();
      lhs = { kind: "binary", op: t.value as BinOp, lhs, rhs };
    }
    return lhs;
  };

  const parseTerm = (): Ast => {
    let lhs = parseUnary();
    while (true) {
      const t = peek();
      if (t.kind !== "sym" || (t.value !== "*" && t.value !== "/" && t.value !== "%")) break;
      pos++;
      const rhs = parseUnary();
      lhs = { kind: "binary", op: t.value as BinOp, lhs, rhs };
    }
    return lhs;
  };

  const parseUnary = (): Ast => {
    const t = peek();
    if (t.kind === "sym" && (t.value === "-" || t.value === "+")) {
      pos++;
      const arg = parseUnary();
      return { kind: "unary", op: t.value as UnOp, arg };
    }
    return parsePower();
  };

  const parsePower = (): Ast => {
    const lhs = parseAtom();
    const t = peek();
    if (t.kind === "sym" && t.value === "^") {
      pos++;
      const rhs = parseUnary(); // right-assoc, tighter than unary
      return { kind: "binary", op: "^", lhs, rhs };
    }
    return lhs;
  };

  const parseAtom = (): Ast => {
    const t = peek();
    if (t.kind === "num") {
      pos++;
      return { kind: "num", value: t.value };
    }
    if (t.kind === "sym" && t.value === "(") {
      pos++;
      const e = parseExpr();
      eatSym(")", "')'");
      return e;
    }
    if (t.kind === "ident") {
      pos++;
      const next = peek();
      if (next.kind === "sym" && next.value === "(") {
        pos++;
        const args: Ast[] = [];
        if (!(peek().kind === "sym" && (peek() as { value: string }).value === ")")) {
          args.push(parseExpr());
          while (peek().kind === "sym" && (peek() as { value: string }).value === ",") {
            pos++;
            args.push(parseExpr());
          }
        }
        eatSym(")", "')'");
        return { kind: "call", name: t.value, args };
      }
      return { kind: "ident", name: t.value };
    }
    throw new ParseError("expected number, identifier, or '('", t.offset);
  };

  const ast = parseExpr();
  const tail = peek();
  if (tail.kind !== "end") throw new ParseError("unexpected trailing input", tail.offset);
  return ast;
}

// ---------------------------------------------------------------------------
// Eval
// ---------------------------------------------------------------------------

export function freeVars(ast: Ast): string[] {
  const out: string[] = [];
  const walk = (a: Ast) => {
    switch (a.kind) {
      case "num":
        return;
      case "ident":
        if (a.name !== "pi" && a.name !== "tau" && a.name !== "e" && !out.includes(a.name))
          out.push(a.name);
        return;
      case "binary":
        walk(a.lhs);
        walk(a.rhs);
        return;
      case "unary":
        walk(a.arg);
        return;
      case "call":
        a.args.forEach(walk);
        return;
    }
  };
  walk(ast);
  return out;
}

export function evalAst(ast: Ast, env: Record<string, number>): number {
  switch (ast.kind) {
    case "num":
      return ast.value;
    case "ident":
      if (ast.name === "pi") return Math.PI;
      if (ast.name === "tau") return Math.PI * 2;
      if (ast.name === "e") return Math.E;
      if (!(ast.name in env)) throw new EvalError(`undefined variable: ${ast.name}`);
      return env[ast.name];
    case "binary": {
      const l = evalAst(ast.lhs, env);
      const r = evalAst(ast.rhs, env);
      switch (ast.op) {
        case "+":
          return l + r;
        case "-":
          return l - r;
        case "*":
          return l * r;
        case "/":
          if (r === 0) throw new EvalError("math domain error: division by zero");
          return l / r;
        case "%":
          if (r === 0) throw new EvalError("math domain error: modulo by zero");
          // rem_euclid equivalent
          return ((l % r) + r) % r;
        case "^":
          return Math.pow(l, r);
      }
      return 0;
    }
    case "unary": {
      const v = evalAst(ast.arg, env);
      return ast.op === "-" ? -v : v;
    }
    case "call":
      return callBuiltin(ast.name, ast.args.map((a) => evalAst(a, env)));
  }
}

function callBuiltin(name: string, v: number[]): number {
  const arity = (n: number) => {
    if (v.length !== n)
      throw new EvalError(`function '${name}' expected ${n} argument(s), got ${v.length}`);
  };
  switch (name) {
    case "sin":
      arity(1);
      return Math.sin(v[0]);
    case "cos":
      arity(1);
      return Math.cos(v[0]);
    case "tan":
      arity(1);
      return Math.tan(v[0]);
    case "asin":
      arity(1);
      return Math.asin(v[0]);
    case "acos":
      arity(1);
      return Math.acos(v[0]);
    case "atan":
      arity(1);
      return Math.atan(v[0]);
    case "atan2":
      arity(2);
      return Math.atan2(v[0], v[1]);
    case "sqrt":
      arity(1);
      if (v[0] < 0) throw new EvalError("math domain error: sqrt of negative");
      return Math.sqrt(v[0]);
    case "abs":
      arity(1);
      return Math.abs(v[0]);
    case "floor":
      arity(1);
      return Math.floor(v[0]);
    case "ceil":
      arity(1);
      return Math.ceil(v[0]);
    case "round":
      arity(1);
      // Match Rust's f64::round (ties away from zero).
      return Math.sign(v[0]) * Math.round(Math.abs(v[0]));
    case "ln":
    case "log":
      arity(1);
      if (v[0] <= 0) throw new EvalError("math domain error: log of non-positive");
      return Math.log(v[0]);
    case "log2":
      arity(1);
      if (v[0] <= 0) throw new EvalError("math domain error: log2 of non-positive");
      return Math.log2(v[0]);
    case "exp":
      arity(1);
      return Math.exp(v[0]);
    case "pow":
      arity(2);
      return Math.pow(v[0], v[1]);
    case "min":
      arity(2);
      return Math.min(v[0], v[1]);
    case "max":
      arity(2);
      return Math.max(v[0], v[1]);
    case "deg":
      arity(1);
      return (v[0] * 180) / Math.PI;
    case "rad":
      arity(1);
      return (v[0] * Math.PI) / 180;
    default:
      throw new EvalError(`unknown function: ${name}`);
  }
}

/** Parse and evaluate in one shot. */
export function evaluate(source: string, env: Record<string, number>): number {
  return evalAst(parse(source), env);
}

// ---------------------------------------------------------------------------
// Document resolution
// ---------------------------------------------------------------------------

/** Parse a binding key `"{nodeId}:{field_path}"`. */
export function parseBindingKey(raw: string): { nodeId: string; fieldPath: string } | null {
  const idx = raw.indexOf(":");
  if (idx <= 0) return null;
  return { nodeId: raw.slice(0, idx), fieldPath: raw.slice(idx + 1) };
}

/** Resolve all parameters into a concrete environment. Throws on cycle. */
export function resolveParameters(
  params: Record<string, Parameter> | undefined,
): Record<string, number> {
  if (!params) return {};
  // Parse each formula once and build a dep graph.
  const asts: Record<string, Ast | null> = {};
  for (const [name, p] of Object.entries(params)) {
    if (typeof p.value === "number") {
      asts[name] = null;
    } else {
      asts[name] = parse(p.value);
    }
  }
  const deps: Record<string, string[]> = {};
  for (const [name, ast] of Object.entries(asts)) {
    const refs: string[] = [];
    if (ast) {
      for (const v of freeVars(ast)) {
        if (v in params) refs.push(v);
      }
    }
    deps[name] = refs;
  }
  // DFS topo-sort with cycle detection.
  const marks: Record<string, "white" | "gray" | "black"> = {};
  for (const name of Object.keys(params)) marks[name] = "white";
  const order: string[] = [];
  const stack: string[] = [];
  const visit = (node: string) => {
    if (marks[node] === "black") return;
    if (marks[node] === "gray") {
      const idx = stack.indexOf(node);
      const path = [...stack.slice(idx), node];
      throw new EvalError(`cycle in parameter dependencies: ${path.join(" → ")}`);
    }
    marks[node] = "gray";
    stack.push(node);
    for (const child of deps[node] ?? []) visit(child);
    stack.pop();
    marks[node] = "black";
    order.push(node);
  };
  for (const name of Object.keys(params)) visit(name);
  const env: Record<string, number> = {};
  for (const name of order) {
    const p = params[name];
    if (typeof p.value === "number") {
      env[name] = p.value;
    } else {
      env[name] = evalAst(asts[name]!, env);
    }
  }
  return env;
}

/** Resolve a single Expr against a parameter env. Returns NaN on error. */
export function evalExprSafe(expr: Expr, env: Record<string, number>): number {
  try {
    return typeof expr === "number" ? expr : evaluate(expr, env);
  } catch {
    return Number.NaN;
  }
}

/**
 * Produce a deep-cloned document with parameters resolved and bindings
 * applied to concrete fields. The returned doc is safe to hand to the
 * WASM kernel or the TS fallback evaluator — it contains no expressions.
 *
 * If the document has no parameters and no bindings, a cheap shallow
 * clone is returned unchanged.
 */
export function resolveDocument(doc: Document): {
  doc: Document;
  env: Record<string, number>;
} {
  const hasParams = !!doc.parameters && Object.keys(doc.parameters).length > 0;
  const hasBindings = !!doc.bindings && Object.keys(doc.bindings).length > 0;
  if (!hasParams && !hasBindings) {
    return { doc, env: {} };
  }
  const env = resolveParameters(doc.parameters);
  const cloned: Document = structuredClone(doc);
  // Deterministic iteration: sort by nodeId then fieldPath.
  const entries: Array<[string, Expr]> = Object.entries(cloned.bindings ?? {});
  entries.sort(([a], [b]) => {
    const ka = parseBindingKey(a);
    const kb = parseBindingKey(b);
    if (!ka || !kb) return a.localeCompare(b);
    const nidA = Number(ka.nodeId);
    const nidB = Number(kb.nodeId);
    if (nidA !== nidB) return nidA - nidB;
    return ka.fieldPath.localeCompare(kb.fieldPath);
  });
  for (const [key, expr] of entries) {
    const parsed = parseBindingKey(key);
    if (!parsed) continue;
    const value = typeof expr === "number" ? expr : evaluate(expr, env);
    applyBinding(cloned, parsed.nodeId, parsed.fieldPath, value);
  }
  return { doc: cloned, env };
}

/** Mutate `node.op` so that the concrete field at `fieldPath` becomes `value`. */
function applyBinding(doc: Document, nodeId: string, fieldPath: string, value: number): void {
  const node = doc.nodes[nodeId];
  if (!node) throw new EvalError(`binding references missing node '${nodeId}'`);
  const op = node.op as unknown as Record<string, unknown>;

  // Helper: write into nested object using dotted path, creating nothing.
  const writeNested = (root: unknown, path: string[]): boolean => {
    let cursor = root as Record<string, unknown>;
    for (let i = 0; i < path.length - 1; i++) {
      const key = path[i];
      if (cursor == null || typeof cursor !== "object" || !(key in cursor)) return false;
      cursor = cursor[key] as Record<string, unknown>;
    }
    const leaf = path[path.length - 1];
    if (cursor == null || typeof cursor !== "object") return false;
    cursor[leaf] = value;
    return true;
  };

  // Handle optional scalar fields that may not yet exist on the op.
  const leaf = fieldPath;
  if (!fieldPath.includes(".")) {
    // Integer-like field handling for segments / count — round and clamp.
    if (leaf === "segments" || leaf === "count") {
      op[leaf] = Math.max(0, Math.round(value));
    } else {
      op[leaf] = value;
    }
    return;
  }
  const parts = fieldPath.split(".");
  const ok = writeNested(op, parts);
  if (!ok) {
    throw new EvalError(
      `binding '${nodeId}:${fieldPath}' — field path not valid on op type '${String(
        (op as { type?: unknown }).type,
      )}'`,
    );
  }
}
