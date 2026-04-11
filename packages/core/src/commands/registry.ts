import type { ToolSchemaEntry, AnthropicTool } from "./types.js";
import type { SelectionContext } from "../stores/chat-store.js";

export class CommandRegistry {
  private schemas: ToolSchemaEntry[] = [];
  private typeCatalogCache: string | null = null;

  /** Load schemas from WASM JSON string. */
  loadSchemas(json: string): void {
    this.schemas = JSON.parse(json) as ToolSchemaEntry[];
    this.typeCatalogCache = null;
  }

  /** Get all loaded schema entries. */
  getSchemas(): ToolSchemaEntry[] {
    return this.schemas;
  }

  /** Get the type enum values for the create tool. */
  getTypeEnum(): string[] {
    return this.schemas.map((s) => s.name);
  }

  /** Generate the four CRUD tool definitions in Anthropic format. */
  toAnthropicTools(): AnthropicTool[] {
    const typeEnum = this.getTypeEnum();

    return [
      {
        name: "create",
        description:
          "Create a new CAD feature. Use 'type' to specify the kind and 'params' for its parameters. See the type catalog in the system prompt for available types and their parameters.",
        input_schema: {
          type: "object",
          properties: {
            type: {
              type: "string",
              enum: typeEnum,
              description: "The CsgOp type to create.",
            },
            params: {
              type: "object",
              description:
                "Parameters for the specified type. See type catalog in system prompt.",
            },
            parent_part_id: {
              type: "string",
              description:
                "If provided, appends the feature to this existing part instead of creating a new one.",
            },
          },
          required: ["type", "params"],
        },
      },
      {
        name: "read",
        description:
          "Inspect the current document. Without part_id, lists all parts. With part_id, returns full feature tree and parameters for that part.",
        input_schema: {
          type: "object",
          properties: {
            part_id: {
              type: "string",
              description: "Part ID to inspect. Omit to list all parts.",
            },
          },
        },
      },
      {
        name: "update",
        description:
          "Update parameters on an existing node. Pass only the fields you want to change.",
        input_schema: {
          type: "object",
          properties: {
            node_id: {
              type: "string",
              description: "The node ID to update.",
            },
            params: {
              type: "object",
              description: "Partial parameter object. Only provided fields are changed.",
            },
          },
          required: ["node_id", "params"],
        },
      },
      {
        name: "delete",
        description: "Delete a part from the document.",
        input_schema: {
          type: "object",
          properties: {
            part_id: {
              type: "string",
              description: "The part ID to delete.",
            },
          },
          required: ["part_id"],
        },
      },
    ];
  }

  /** Build the type catalog section for the system prompt. Cached until schemas change. */
  getTypeCatalog(): string {
    if (this.typeCatalogCache) return this.typeCatalogCache;

    const lines: string[] = ["## Type Catalog", ""];
    for (const schema of this.schemas) {
      lines.push(`### ${schema.name} (${schema.category})`);
      lines.push(schema.description);
      if (schema.ai_hint) lines.push(schema.ai_hint);

      const props = (schema.input_schema as { properties?: Record<string, Record<string, unknown>> }).properties;
      if (props && Object.keys(props).length > 0) {
        lines.push("Parameters:");
        for (const [key, prop] of Object.entries(props)) {
          const type = (prop.type as string) || "object";
          const desc = (prop.description as string) || "";
          lines.push(`- ${key}: ${type}${desc ? " — " + desc : ""}`);
        }
      }
      lines.push("");
    }

    this.typeCatalogCache = lines.join("\n");
    return this.typeCatalogCache;
  }

  /** Build the full system prompt with type catalog and document state. */
  buildSystemPrompt(
    parts: Array<{ id: string; name: string; kind: string; nodes?: Array<{ nodeId: string; type: string; params: Record<string, unknown> }> }>,
    selection?: SelectionContext[],
  ): string {
    const sections: string[] = [
      "You are vcad's AI assistant — a parametric CAD copilot.",
      "Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.",
      "You have four tools: create, read, update, delete. Be concise.",
      "",
      "When asked to create or modify geometry, use the available tools. After a tool call, briefly confirm what you did.",
      'When the user refers to "this" or "it" without specifics, use the selected geometry context provided.',
      "",
      this.getTypeCatalog(),
    ];

    if (parts.length > 0) {
      sections.push("## Current Document");
      sections.push("Parts:");
      for (const part of parts) {
        sections.push(`- ${part.id} "${part.name}" [${part.kind}]`);
        if (part.nodes) {
          for (const node of part.nodes) {
            const paramStr = Object.entries(node.params)
              .map(([k, v]) => `${k}: ${JSON.stringify(v)}`)
              .join(", ");
            sections.push(`  └─ ${node.nodeId} [${node.type}] ${paramStr}`);
          }
        }
      }
      sections.push("");
    }

    if (selection?.length) {
      const selList = selection
        .map((s) => `- ${s.partName} (${s.geometryType}, id: ${s.partId})`)
        .join("\n");
      sections.push(`Selected:\n${selList}`);
    }

    return sections.join("\n");
  }
}

/** Singleton registry instance. */
export const commandRegistry = new CommandRegistry();
