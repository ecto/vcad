/** Mirrors Rust ToolSchemaEntry — parsed from WASM JSON at init. */
export interface ToolSchemaEntry {
  name: string;
  description: string;
  category: string;
  aiHint?: string;
  inputSchema: Record<string, unknown>;
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
}

/** Anthropic tool definition format. */
export interface AnthropicTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}
