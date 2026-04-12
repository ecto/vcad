/** Mirrors Rust ToolSchemaEntry — parsed from WASM JSON at init. */
export interface ToolSchemaEntry {
  name: string;
  description: string;
  category: string;
  ai_hint?: string;
  input_schema: Record<string, unknown>;
}

/** Renderable piece of a tool call summary sentence. */
export type SummarySegment =
  | { type: "text"; text: string }
  | { type: "partLink"; partId: string; name: string };

/** Optional rich display payload attached to a successful execution. */
export interface ExecutionDisplay {
  /** The at-rest summary sentence, as template segments (text + clickable part links). */
  summary: SummarySegment[];
  /** Human-readable parameter list for the expanded detail view. */
  fields?: Array<{ label: string; value: string }>;
  /** Part IDs touched by this call — used by the chip to highlight on hover. */
  affectedPartIds?: string[];
}

/** Result of executing a CRUD tool. */
export interface ExecutionResult {
  status: "success" | "error";
  /** Human-readable summary returned to the AI. */
  result: string;
  /** Part ID if a part was created or modified. */
  partId?: string;
  /** Node ID if a node was created or modified. */
  nodeId?: string;
  /** Optional rich display payload for UI chips. Absent = UI falls back to `result`. */
  display?: ExecutionDisplay;
  /** Duration of the execution in milliseconds, populated by executeCrud wrapper. */
  duration?: number;
}

/** Anthropic tool definition format. */
export interface AnthropicTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}
