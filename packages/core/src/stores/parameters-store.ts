/**
 * Client-side store for document parameters and expression bindings.
 *
 * Parameters and bindings live **outside** the CRDT document for v1 — they
 * are not yet part of the collaborative schema. The
 * [`mergeParametersIntoDocument`] helper injects them back onto a
 * [`Document`](@vcad/ir) immediately before evaluation, so the Rust kernel
 * and the TS fallback both see a parametric doc.
 *
 * When the CRDT schema learns about parameters, this store's state can be
 * migrated in-place by replacing `set` with CRDT-backed mutations.
 */

import { create } from "zustand";
import type { Bindings, Document, Expr, Parameter } from "@vcad/ir";

export interface ParametersState {
  /** Flat map of parameter name → Parameter definition. */
  parameters: Record<string, Parameter>;
  /** Sidecar map `"{nodeId}:{field.path}"` → Expr. */
  bindings: Bindings;

  // Mutations
  setParameter: (name: string, parameter: Parameter) => void;
  updateParameterValue: (name: string, value: Expr) => void;
  renameParameter: (oldName: string, newName: string) => void;
  removeParameter: (name: string) => void;

  setBinding: (nodeId: string | number, fieldPath: string, expr: Expr) => void;
  removeBinding: (nodeId: string | number, fieldPath: string) => void;

  /** Wipe everything (used when loading a new document). */
  reset: (payload?: { parameters?: Record<string, Parameter>; bindings?: Bindings }) => void;
}

function bindingKey(nodeId: string | number, fieldPath: string): string {
  return `${nodeId}:${fieldPath}`;
}

/**
 * Rewrite every binding formula so that references to `oldName` become
 * `newName`. Uses the expression parser so we do NOT string-replace inside
 * numeric literals or inside other identifiers that contain the old name
 * as a substring.
 *
 * Lazily imported to avoid pulling @vcad/engine at module load.
 */
async function renameInBindings(
  bindings: Bindings,
  oldName: string,
  newName: string,
): Promise<Bindings> {
  if (oldName === newName) return bindings;
  const { parseExpression } = await import("@vcad/engine");
  const out: Bindings = {};
  for (const [key, expr] of Object.entries(bindings)) {
    if (typeof expr === "number") {
      out[key] = expr;
      continue;
    }
    try {
      const ast = parseExpression(expr);
      out[key] = renameInAst(ast, oldName, newName);
    } catch {
      // Leave malformed bindings untouched so the user sees the parse
      // error in the UI rather than silently losing their formula.
      out[key] = expr;
    }
  }
  return out;
}

function renameInAst(
  ast: import("@vcad/engine").ExpressionAst,
  oldName: string,
  newName: string,
): string {
  switch (ast.kind) {
    case "num":
      return formatNumber(ast.value);
    case "ident":
      return ast.name === oldName ? newName : ast.name;
    case "unary":
      return `(${ast.op}${renameInAst(ast.arg, oldName, newName)})`;
    case "binary":
      return `(${renameInAst(ast.lhs, oldName, newName)} ${ast.op} ${renameInAst(ast.rhs, oldName, newName)})`;
    case "call":
      return `${ast.name}(${ast.args.map((a) => renameInAst(a, oldName, newName)).join(", ")})`;
  }
}

function formatNumber(n: number): string {
  // Preserve integer-looking numbers as integers; fall back to exponential
  // for very small / large magnitudes. This keeps rewrites readable.
  if (Number.isInteger(n)) return String(n);
  return String(n);
}

export const useParametersStore = create<ParametersState>((set, get) => ({
  parameters: {},
  bindings: {},

  setParameter: (name, parameter) => {
    set((s) => ({
      parameters: { ...s.parameters, [name]: parameter },
    }));
  },

  updateParameterValue: (name, value) => {
    set((s) => {
      const existing = s.parameters[name] ?? ({ value } as Parameter);
      return {
        parameters: { ...s.parameters, [name]: { ...existing, value } },
      };
    });
  },

  renameParameter: (oldName, newName) => {
    const state = get();
    if (oldName === newName) return;
    if (!(oldName in state.parameters)) return;
    if (newName in state.parameters) {
      console.warn(`[parameters] ${newName} already exists; rename aborted`);
      return;
    }
    const nextParameters: Record<string, Parameter> = {};
    for (const [k, v] of Object.entries(state.parameters)) {
      nextParameters[k === oldName ? newName : k] = v;
    }
    set({ parameters: nextParameters });
    // Fire-and-forget: rebind references once the parser resolves.
    void renameInBindings(state.bindings, oldName, newName).then((next) =>
      set({ bindings: next }),
    );
  },

  removeParameter: (name) => {
    set((s) => {
      const { [name]: _, ...rest } = s.parameters;
      return { parameters: rest };
    });
  },

  setBinding: (nodeId, fieldPath, expr) => {
    set((s) => ({
      bindings: { ...s.bindings, [bindingKey(nodeId, fieldPath)]: expr },
    }));
  },

  removeBinding: (nodeId, fieldPath) => {
    set((s) => {
      const key = bindingKey(nodeId, fieldPath);
      const { [key]: _, ...rest } = s.bindings;
      return { bindings: rest };
    });
  },

  reset: (payload) => {
    set({
      parameters: payload?.parameters ?? {},
      bindings: payload?.bindings ?? {},
    });
  },
}));

/**
 * Return a shallow clone of `doc` with `parameters` and `bindings`
 * attached, so the engine's resolver sees them. Cheap — only one object
 * spread, no deep clone.
 *
 * Call this right before `engine.evaluate(doc)`.
 */
export function mergeParametersIntoDocument(
  doc: Document,
  parameters: Record<string, Parameter>,
  bindings: Bindings,
): Document {
  const hasParams = Object.keys(parameters).length > 0;
  const hasBindings = Object.keys(bindings).length > 0;
  if (!hasParams && !hasBindings) return doc;
  return {
    ...doc,
    parameters: hasParams ? parameters : doc.parameters,
    bindings: hasBindings ? bindings : doc.bindings,
  };
}
