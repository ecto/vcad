/**
 * Shared MCP tool-result shape + `ok`/`err` helpers.
 *
 * Before the ToolDef registry refactor each tool module carried its own
 * near-identical copy of these (checkpoint, continue-doc, atoms, order,
 * ordering, live-share, verify). They diverged only in two axes — compact vs
 * pretty-printed JSON, and whether the error body is a raw string or an
 * `{error}` envelope — so the primitives below cover both without changing any
 * handler's emitted bytes.
 */

/** The canonical MCP tool-call result. `content` is usually a single text
 *  block of JSON; image tools carry image blocks (cast at the call site). */
export interface ToolResult {
  content: Array<{ type: string; text: string; annotations?: unknown }>;
  isError?: boolean;
  structuredContent?: Record<string, unknown>;
  _meta?: Record<string, unknown>;
}

/** A single JSON text block, optionally pretty-printed and/or flagged as an
 *  error. The one primitive the convenience helpers below are built on. */
export function toolResult(
  body: unknown,
  opts: { pretty?: boolean; isError?: boolean } = {},
): ToolResult {
  const text = opts.pretty
    ? JSON.stringify(body, null, 2)
    : JSON.stringify(body);
  const result: ToolResult = { content: [{ type: "text", text }] };
  if (opts.isError) result.isError = true;
  return result;
}

/** Success result: one compact-JSON text block. */
export function ok(body: unknown): ToolResult {
  return toolResult(body);
}

/** Success result: one pretty-printed (2-space) JSON text block. */
export function okPretty(body: unknown): ToolResult {
  return toolResult(body, { pretty: true });
}

/** Error result: an `{error: message}` JSON body flagged `isError`. */
export function err(message: string): ToolResult {
  return toolResult({ error: message }, { isError: true });
}

/** Error result carrying a raw text body (no JSON envelope), `isError`. */
export function errText(text: string): ToolResult {
  return { isError: true, content: [{ type: "text", text }] };
}
