/**
 * Parametric expression parsing and resolution — thin wrapper over the
 * kernel WASM bindings (`crates/vcad-kernel-wasm/src/expressions.rs`).
 *
 * The grammar and evaluation semantics live in ONE place: the Rust
 * expression parser in `crates/vcad-ir/src/expr_parser.rs` (plus the
 * parameter/binding resolution pass in `vcad-ir::parameters` and
 * `vcad-eval::resolve`). This module only marshals values across the
 * WASM boundary and preserves the historical TS public API (`parse`,
 * `evaluate`, `evalAst`, `freeVars`, `resolveParameters`,
 * `resolveDocument`, `ParseError`, `EvalError`).
 *
 * Requires the kernel WASM singleton to be initialized (`getKernelWasm()`
 * / `Engine.init()`); every entry point throws a descriptive error
 * otherwise. There is deliberately NO TypeScript fallback parser — a
 * second implementation is exactly the bit-for-bit drift hazard this
 * wrapper exists to remove.
 *
 * `freeVars` and `parseBindingKey` are pure structural helpers over data
 * shapes (the wire AST and the `"{nodeId}:{fieldPath}"` key format) and
 * stay in TS — they encode no grammar or math semantics.
 */

import type { Document, Expr, Parameter } from "@vcad/ir";
import { getKernelWasmSync } from "./wasm-singleton.js";

// ---------------------------------------------------------------------------
// AST (wire shape produced by the Rust parser)
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

/** Rust `ParseError` Display: `parse error at offset N: <message>`. */
const PARSE_ERROR_RE = /^parse error at offset (\d+): (.*)$/s;

/**
 * Map an error thrown by a WASM expression binding onto the historical
 * TS error classes. Parse failures keep their byte offset; everything
 * else (undefined variable, arity, math domain, ...) becomes EvalError.
 */
function toExpressionError(e: unknown): Error {
  const message = e instanceof Error ? e.message : String(e);
  const m = PARSE_ERROR_RE.exec(message);
  if (m) return new ParseError(m[2], Number(m[1]));
  return new EvalError(message);
}

// ---------------------------------------------------------------------------
// WASM access
// ---------------------------------------------------------------------------

interface ExpressionWasm {
  exprParse(src: string): Ast;
  exprEvalAst(ast: Ast, env: Record<string, number>): number;
  exprEvaluate(src: string, env: Record<string, number>): number;
  resolveParametersJson(paramsJson: string): string;
  resolveDocumentJson(docJson: string): string;
}

function requireWasm(fn: keyof ExpressionWasm): ExpressionWasm {
  const mod = getKernelWasmSync() as unknown as ExpressionWasm | null;
  if (!mod) {
    throw new Error(
      `${fn}: kernel WASM is not initialized — await getKernelWasm() (or Engine.init()) before using expression APIs`,
    );
  }
  if (typeof mod[fn] !== "function") {
    throw new Error(
      `${fn} is not exported by this kernel WASM build — rebuild packages/kernel-wasm`,
    );
  }
  return mod;
}

// ---------------------------------------------------------------------------
// Parse / eval
// ---------------------------------------------------------------------------

/** Parse an expression string into an AST. Throws ParseError. */
export function parse(src: string): Ast {
  try {
    return requireWasm("exprParse").exprParse(src);
  } catch (e) {
    throw toExpressionError(e);
  }
}

/** Evaluate a parsed AST against an environment. Throws EvalError. */
export function evalAst(ast: Ast, env: Record<string, number>): number {
  try {
    return requireWasm("exprEvalAst").exprEvalAst(ast, env);
  } catch (e) {
    throw toExpressionError(e);
  }
}

/** Parse and evaluate in one shot. Throws ParseError or EvalError. */
export function evaluate(source: string, env: Record<string, number>): number {
  try {
    return requireWasm("exprEvaluate").exprEvaluate(source, env);
  } catch (e) {
    throw toExpressionError(e);
  }
}

/**
 * Collect free identifiers referenced by an AST (excluding the named
 * constants `pi`, `tau`, `e`), in first-appearance order.
 */
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
  if (!params || Object.keys(params).length === 0) return {};
  try {
    const json = requireWasm("resolveParametersJson").resolveParametersJson(
      JSON.stringify(params),
    );
    return JSON.parse(json) as Record<string, number>;
  } catch (e) {
    throw toExpressionError(e);
  }
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
 * Produce a resolved copy of the document: parameters evaluated in
 * dependency order, bindings applied to concrete fields. The returned
 * doc is safe to hand to the WASM kernel or the TS fallback evaluator —
 * it contains no unresolved expressions.
 *
 * If the document has no parameters and no bindings, the original doc
 * is returned unchanged (no round-trip cost).
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
  try {
    const json = requireWasm("resolveDocumentJson").resolveDocumentJson(JSON.stringify(doc));
    return JSON.parse(json) as { doc: Document; env: Record<string, number> };
  } catch (e) {
    throw toExpressionError(e);
  }
}
