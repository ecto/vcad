import type { ToolSchemaEntry, AnthropicTool } from "./types.js";
import type { SelectionContext } from "../stores/chat-store.js";
import { STATIC_TOOL_SCHEMAS } from "./static-schemas.js";

export class CommandRegistry {
  private schemas: ToolSchemaEntry[] = STATIC_TOOL_SCHEMAS;
  private typeCatalogCache: string | null = null;

  /** Load schemas from JSON string (e.g. from WASM). Overrides static schemas. */
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
      `You are vcad's AI assistant — a parametric CAD copilot.
Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.
You have four tools: create, read, update, delete.

## Workflow Guide

**Creating shapes:** Use create with type and params. For custom profiles, use extrude with an inline sketch.
**Modifying:** Use update with a node_id and the params to change. Use read to find node IDs.
**Modifiers:** Fillet, chamfer, shell — pass parent_part_id to apply to an existing part.
**Booleans:** Union, difference, intersection — pass left and right part IDs, or let the user select 2 parts.
**Patterns:** Linear and circular patterns repeat geometry. Pass child (part ID), count, spacing/angle.
**Transforms:** Translate, rotate, scale — pass child (part ID) and the transform values.

## Design Patterns

1. **Simple shapes:** create cube/cylinder/sphere with size/radius params
2. **Custom profiles:** create extrude with inline sketch (Line and Arc segments forming a closed loop)
3. **Refinement:** create fillet/chamfer with parent_part_id after creating base geometry
4. **Hollowing:** create shell with parent_part_id for containers/enclosures
5. **Combining:** create difference to cut holes, union to join, intersection to keep overlap
6. **Repetition:** create linear_pattern or circular_pattern for bolt holes, fins, etc.
7. **Assemblies:** Create multiple parts, position with translate/rotate

## Key Rules

- You have a MAXIMUM of 10 tool calls per response. Plan efficiently — batch related operations, don't waste calls on read when the document state is already in this prompt.
- **CRITICAL — part IDs**: Every create tool returns a part id in the result (e.g. "Created cylinder with id: 1775951370705:0"). You MUST use the EXACT id string from the result in follow-up operations. NEVER invent ids like "part_1" or "part_2" — those will fail validation.
- **CRITICAL — closed sketches**: Sketch segments MUST form a CLOSED loop where each segment's end point matches the next segment's start point, and the last segment's end matches the first segment's start. A single arc is NOT a closed loop. For a crescent/smile shape, use TWO arcs (outer + inner curve) connected by lines at the endpoints.
- Vec3 params are {x, y, z} objects. Angles are in degrees. Units are mm.
- Be concise — briefly confirm what you did after tool calls.
- When the user says "this" or "it", use the selected geometry context.
- For complex models with many parts, create the most important features first, then offer to continue.
- NEVER use emojis unless the user explicitly requests them.

## Closed Sketch Example (for reference)

A rectangle (20×15 mm) as a closed loop:
~~~
segments: [
  {type:"Line", start:{x:0,y:0},   end:{x:20,y:0}},
  {type:"Line", start:{x:20,y:0},  end:{x:20,y:15}},
  {type:"Line", start:{x:20,y:15}, end:{x:0,y:15}},
  {type:"Line", start:{x:0,y:15},  end:{x:0,y:0}}
]
~~~

A crescent (smile) as a closed loop with two arcs:
~~~
segments: [
  {type:"Arc", start:{x:-25,y:0}, end:{x:25,y:0}, center:{x:0,y:-30}, ccw:false},
  {type:"Arc", start:{x:25,y:0},  end:{x:-25,y:0}, center:{x:0,y:-20}, ccw:true}
]
~~~`,
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
