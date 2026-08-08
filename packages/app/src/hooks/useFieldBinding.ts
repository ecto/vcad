import { useCallback, useMemo } from "react";
import { useParametersStore } from "@vcad/core";
import {
  ExpressionParseError,
  evaluateExpression,
  expressionFreeVars,
  parseExpression,
  resolveParameters,
} from "@vcad/engine";

/**
 * Wire one numeric property field to the document's parameter bindings.
 *
 * The property panel's fields were number-only: typing `wall` into a
 * dimension parsed as NaN and snapped back, so the only way to bind a
 * parameter to geometry was to author the document elsewhere. This closes
 * that — the panel is where a parameter meets a field.
 *
 * Returns props to spread onto a `ScrubInput`.
 */
export function useFieldBinding(nodeId: number | string | undefined, fieldPath: string) {
  const bindings = useParametersStore((s) => s.bindings);
  const parameters = useParametersStore((s) => s.parameters);
  const setBinding = useParametersStore((s) => s.setBinding);
  const removeBinding = useParametersStore((s) => s.removeBinding);

  const key = nodeId == null ? null : `${nodeId}:${fieldPath}`;
  const bound = key ? bindings[key] : undefined;
  const expression = typeof bound === "string" ? bound : null;

  // Names a formula is allowed to reference.
  const known = useMemo(() => new Set(Object.keys(parameters)), [parameters]);

  const validateExpression = useCallback(
    (expr: string): string | null => {
      let ast;
      try {
        ast = parseExpression(expr);
      } catch (e) {
        return e instanceof ExpressionParseError ? e.message : "invalid expression";
      }
      // An unknown name is the common typo, and the one worth naming
      // precisely — "undefined parameter 'wal'" beats "invalid expression".
      const unknown = [...expressionFreeVars(ast)].filter((v) => !known.has(v));
      if (unknown.length > 0) {
        return `unknown parameter${unknown.length > 1 ? "s" : ""}: ${unknown.join(", ")}`;
      }
      // A formula that cannot be resolved right now (a cycle through
      // another parameter) would evaluate to NaN and quietly break geometry.
      try {
        const env = resolveParameters(parameters);
        const missing = [...expressionFreeVars(ast)].filter(
          (v) => !Number.isFinite(env[v]),
        );
        if (missing.length > 0) {
          return `parameter${missing.length > 1 ? "s" : ""} do not resolve: ${missing.join(", ")}`;
        }
      } catch {
        return "parameters do not resolve (cycle?)";
      }
      return null;
    },
    [known, parameters],
  );

  const onBind = useCallback(
    (expr: string | null) => {
      if (nodeId == null) return;
      if (expr === null) removeBinding(nodeId, fieldPath);
      else setBinding(nodeId, fieldPath, expr);
    },
    [nodeId, fieldPath, setBinding, removeBinding],
  );

  // What the formula currently evaluates to. The document node still holds
  // the *unbound* literal — bindings resolve at evaluation time — so a bound
  // field that displayed `value` would show a stale number forever.
  const resolvedValue = useMemo(() => {
    if (!expression) return undefined;
    try {
      const env = resolveParameters(parameters);
      const v = evaluateExpression(expression, env);
      return Number.isFinite(v) ? v : undefined;
    } catch {
      return undefined;
    }
  }, [expression, parameters]);

  return {
    expression,
    resolvedValue,
    // Only offer binding when the document actually has parameters —
    // otherwise every typo becomes "unknown parameter" noise on a document
    // where binding was never possible.
    onBind: known.size > 0 ? onBind : undefined,
    validateExpression,
  };
}

/**
 * Override any bound fields of a node with what their expressions currently
 * evaluate to.
 *
 * The document stores a node's *unbound* literal — bindings resolve at
 * evaluation time — so anything reading `document.nodes[...]` directly shows
 * a number that never moves once the field is bound. Pass the raw values in,
 * get the effective ones out.
 *
 * A pure function rather than a hook so callers can use it inside a `useMemo`
 * over many nodes at once.
 */
export function resolveBoundFields(
  nodeId: number | string,
  raw: Record<string, number>,
  parameters: Record<string, import("@vcad/ir").Parameter>,
  bindings: Record<string, import("@vcad/ir").Expr>,
): Record<string, number> {
  let env: Record<string, number> | null = null;
  const out: Record<string, number> = { ...raw };
  for (const field of Object.keys(raw)) {
    const expr = bindings[`${nodeId}:${field}`];
    if (expr == null) continue;
    if (typeof expr === "number") {
      out[field] = expr;
      continue;
    }
    try {
      env ??= resolveParameters(parameters);
      const v = evaluateExpression(expr, env);
      if (Number.isFinite(v)) out[field] = v;
    } catch {
      // Leave the raw value: a broken formula should not blank a dimension.
    }
  }
  return out;
}
