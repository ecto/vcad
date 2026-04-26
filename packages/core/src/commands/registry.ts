import type { ToolSchemaEntry, AnthropicTool } from "./types.js";
import type { SelectionContext } from "../stores/chat-store.js";
import { STATIC_TOOL_SCHEMAS } from "./static-schemas.js";

/**
 * Optional wasm-bindgen surface from `vcad-kernel-wasm` that mirrors the
 * TS tool builder + prompt builder. When the web kernel loads, bootstrap
 * wires the module in via `commandRegistry.setWasm(wasm)` — from that
 * point on, `toAnthropicTools` and `buildSystemPrompt` delegate to Rust
 * so the TS and TUI render byte-identical payloads.
 *
 * Keeping the TS fallback means:
 *  - tests that don't load wasm still run
 *  - the static-schemas bootstrap still works before the module resolves
 *  - a future contributor who forgets to call `setWasm` gets a valid,
 *    slightly-stale prompt instead of a crash
 */
export interface ChatWasmBindings {
  get_anthropic_tools_json(): string;
  build_chat_system_prompt(partsJson: string, selectionJson: string): string;
  /** Plan a CRUD tool call against a document snapshot. Returns a JSON
   *  `PlannedResponse`. Not every kernel-wasm build exposes this (it
   *  landed after the other two bindings), so callers must null-check. */
  plan_chat_tool?(tool: string, argsJson: string, docJson: string): string;
}

/** Rust-side `ToolOutcome` enum mirrored for TS dispatch. Every variant
 *  corresponds to a specific docstore mutation path on the web. */
export type ToolOutcome =
  | {
      kind: "add_feature";
      /** The new feature's op, ready to hand to `engine.add_feature`. */
      op: Record<string, unknown>;
      name?: string;
      parent_part_id?: string;
    }
  | { kind: "update_params"; node_id: string; params: Record<string, unknown> }
  | { kind: "remove_part"; part_id: string }
  | { kind: "set_part_material"; part_id: string; material: string };

/** Rust-side `PlannedResponse` mirrored for TS. */
export interface PlannedResponse {
  status: "success" | "error";
  result: string;
  outcome?: ToolOutcome;
}

export class CommandRegistry {
  private schemas: ToolSchemaEntry[] = STATIC_TOOL_SCHEMAS;
  private typeCatalogCache: string | null = null;
  private wasm: ChatWasmBindings | null = null;

  /** Load schemas from JSON string (e.g. from WASM). Overrides static schemas. */
  loadSchemas(json: string): void {
    this.schemas = JSON.parse(json) as ToolSchemaEntry[];
    this.typeCatalogCache = null;
  }

  /**
   * Register the kernel-wasm module so the registry can delegate
   * `toAnthropicTools` / `buildSystemPrompt` to the Rust implementation
   * in `vcad-chat`. Both sides produce identical output; the delegation
   * prevents long-term drift when either half changes.
   *
   * Safe to call multiple times — last call wins. Callers pass `null`
   * to drop the binding (used by tests that want to exercise the TS
   * fallback explicitly).
   */
  setWasm(wasm: ChatWasmBindings | null): void {
    this.wasm = wasm;
  }

  /**
   * Plan a CRUD tool call via the Rust executor. Returns `null` if the
   * wasm binding isn't present (old kernel build, test harness, etc.)
   * so callers can fall back to the TS `executeCrud` path.
   *
   * `docJson` is a `JSON.stringify(document)` of the current store
   * snapshot — the planner needs it for id/existence validation.
   */
  planCrud(
    tool: string,
    args: Record<string, unknown>,
    docJson: string,
  ): PlannedResponse | null {
    if (!this.wasm?.plan_chat_tool) return null;
    try {
      const raw = this.wasm.plan_chat_tool(tool, JSON.stringify(args), docJson);
      return JSON.parse(raw) as PlannedResponse;
    } catch {
      return null;
    }
  }

  /** Get all loaded schema entries. */
  getSchemas(): ToolSchemaEntry[] {
    return this.schemas;
  }

  /** Get the type enum values for the create tool. */
  getTypeEnum(): string[] {
    return this.schemas.map((s) => s.name);
  }

  /**
   * Generate the five CRUD tool definitions in Anthropic format. When
   * the kernel-wasm binding is wired (the common case in the running
   * web app), delegates to `vcad_chat::anthropic_tools` so the TS and
   * the Rust TUI emit byte-identical tool payloads. Falls back to the
   * inline hand-written definitions below during bootstrap before wasm
   * has loaded, and in tests that don't mount the kernel.
   */
  toAnthropicTools(): AnthropicTool[] {
    if (this.wasm) {
      try {
        const rustTools = JSON.parse(
          this.wasm.get_anthropic_tools_json(),
        ) as AnthropicTool[];
        // Append any TS-side tools that the Rust side doesn't know about
        // yet. This lets us ship new tools (e.g. camera tools) without a
        // lockstep Rust edit; the TS fallback below is the source of
        // truth, and duplicates from Rust win.
        const rustNames = new Set(rustTools.map((t) => t.name));
        const extras = this.tsFallbackTools().filter((t) => !rustNames.has(t.name));
        return extras.length > 0 ? [...rustTools, ...extras] : rustTools;
      } catch {
        // Fall through to the TS fallback — better a stale copy than a
        // runtime crash if wasm is in a bad state.
      }
    }
    return this.tsFallbackTools();
  }

  /** Hand-written tool definitions used when wasm isn't loaded. */
  private tsFallbackTools(): AnthropicTool[] {
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
            name: {
              type: "string",
              description:
                "Optional human-readable name for this feature (e.g. 'Front Wheel', 'Top Tube', 'Seat Post'). Strongly recommended — makes the feature tree readable. Keep short (1-3 words).",
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
      {
        name: "set_material",
        description:
          "Set the material/color of one OR MANY parts in a single call. " +
          "Provide ONE of: `part_id` (single), `part_ids` (explicit array), or " +
          "`selector` (match by kind/name). Prefer the bulk forms — assigning " +
          "the same material to 18 parts via 18 calls is wasteful when one " +
          "selector call would do it.",
        input_schema: {
          type: "object",
          properties: {
            part_id: {
              type: "string",
              description: "Single part ID to set the material on.",
            },
            part_ids: {
              type: "array",
              items: { type: "string" },
              description: "Array of part IDs — applies the material to each.",
            },
            selector: {
              type: "object",
              description:
                "Match many parts at once. `by` is one of: 'kind' | 'name_prefix' | 'name_contains' | 'name_equals'. `value` is matched case-insensitively. Example: { by: 'name_prefix', value: 'Spoke' }.",
              properties: {
                by: {
                  type: "string",
                  enum: ["kind", "name_prefix", "name_contains", "name_equals"],
                },
                value: { type: "string" },
              },
              required: ["by", "value"],
            },
            material: {
              type: "string",
              description: "Material preset key (e.g. 'aluminum', 'gold', 'oak', 'abs-red').",
            },
          },
          required: ["material"],
        },
      },
      {
        name: "focus_part",
        description:
          "Point your camera at a part so the user can see what you're working on. Also highlights the part in your participant color as an attention cue. Use this after creating or modifying something you want the user to notice.",
        input_schema: {
          type: "object",
          properties: {
            part_id: {
              type: "string",
              description: "The part ID to focus on.",
            },
          },
          required: ["part_id"],
        },
      },
      {
        name: "frame_all",
        description:
          "Frame the entire scene in your camera view. Useful for taking a step back to show the user the whole model at once.",
        input_schema: {
          type: "object",
          properties: {},
        },
      },
      {
        name: "set_view",
        description:
          "Snap your camera to a preset viewing angle: iso, hero, top, bottom, front, back, left, or right. Frames the current scene bounding box from the chosen direction.",
        input_schema: {
          type: "object",
          properties: {
            name: {
              type: "string",
              enum: ["iso", "hero", "top", "bottom", "front", "back", "left", "right"],
              description: "The snap view to use.",
            },
          },
          required: ["name"],
        },
      },
      {
        name: "tube",
        description:
          "Create a cylindrical pipe/tube between two world-space points. One call, no trigonometry: pass start and end and radius, and the tube is created with correct orientation, length, and cross-section. Strongly preferred over manually building an extrude with perpendicular-basis vectors — this is the right tool for frame tubes, pipes, handlebars, axles, spindles, and similar segments.",
        input_schema: {
          type: "object",
          properties: {
            start: {
              type: "object",
              properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } },
              description: "World-space start point of the tube centerline.",
              required: ["x", "y", "z"],
            },
            end: {
              type: "object",
              properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } },
              description: "World-space end point of the tube centerline.",
              required: ["x", "y", "z"],
            },
            radius: { type: "number", description: "Tube radius in mm (default 5)." },
            arc_segments: { type: "integer", description: "Circumferential resolution (default 16)." },
            name: { type: "string", description: "Short descriptive name for the tube." },
          },
          required: ["start", "end"],
        },
      },
      {
        name: "polyline_tube",
        description:
          "Create a chain of tubes through a sequence of world-space points. Each consecutive pair of points becomes one tube segment (a separate part — not unioned). Perfect for bike frames, piping, cable runs, and any multi-segment pipe run where the joints are points rather than vectors.",
        input_schema: {
          type: "object",
          properties: {
            points: {
              type: "array",
              description: "Ordered list of world-space points the tube passes through. Must have ≥2 entries.",
              items: {
                type: "object",
                properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } },
                required: ["x", "y", "z"],
              },
            },
            radius: { type: "number", description: "Tube radius in mm (default 5)." },
            arc_segments: { type: "integer", description: "Circumferential resolution (default 16)." },
            name: { type: "string", description: "Base name (segments named 'Base 1', 'Base 2', ...)." },
          },
          required: ["points"],
        },
      },
      {
        name: "inspect_part",
        description:
          "Read a part's current world-space geometry: bounding box, size, center, translate, rotation, material, and the set of named anchors (center/min/max/top/bottom/front/back/left/right) you can pass to `place`. Use this to verify what the scene looks like after a tool call without spending tokens on a screenshot — it's much cheaper than screenshot_viewport. Result is JSON.",
        input_schema: {
          type: "object",
          properties: {
            part_id: {
              type: "string",
              description: "The part ID to inspect.",
            },
          },
          required: ["part_id"],
        },
      },
      {
        name: "place",
        description:
          "Position a part by anchor: moves the part so that its `from` anchor lands on the `to` anchor. Both anchors can be either a named anchor on this or another part (center/min/max/top/bottom/front/back/left/right), or an explicit world-space {x,y,z} point. Prefer this over manual translate/rotate when you're aligning to another part — it remains correct when upstream dimensions change.",
        input_schema: {
          type: "object",
          properties: {
            part_id: { type: "string", description: "The part to move." },
            from: {
              description: "The anchor on this part to place. Either a named anchor string, a {x,y,z} point, or {part, anchor}. Defaults to 'center'.",
            },
            to: {
              description: "The destination. Either a named anchor string (resolved on this part), a {x,y,z} world point, or {part, anchor}.",
            },
          },
          required: ["part_id", "to"],
        },
      },
      {
        name: "describe_scene",
        description:
          "One-call snapshot of the whole scene (or a subset of parts): each part's world-space bbox, center, translate, rotate, and material as JSON. Use this INSTEAD of a chain of `inspect_part` calls when you need positions for several parts at once — one turn, no round-trips. Also use it after a batch of edits to verify where everything ended up.",
        input_schema: {
          type: "object",
          properties: {
            part_ids: {
              type: "array",
              items: { type: "string" },
              description: "Optional list of part IDs to describe. Omit to describe every part.",
            },
            limit: {
              type: "integer",
              description: "Cap the number of parts returned when no part_ids are provided (default 100).",
            },
          },
        },
      },
      {
        name: "search_parts",
        description:
          "Search the stdlib parts library (fasteners, bearings, …) for parts matching a query. Free-text matches across name, category, synonyms, and catalog part numbers (McMaster / ISO / DIN). Returns an array of {id, name, category, params, xrefs} — use the `id` with `place_part`. Part numbers like `91290A320` or `ISO 4762` match directly.",
        input_schema: {
          type: "object",
          properties: {
            query: {
              type: "string",
              description: "Free-text search query (part name, McMaster number, ISO/DIN reference, synonym, or category).",
            },
            category: {
              type: "string",
              description: "Optional category filter (e.g. 'Fasteners', 'Bearings').",
            },
            limit: {
              type: "integer",
              description: "Maximum number of results (default 10).",
            },
          },
        },
      },
      {
        name: "place_part",
        description:
          "Insert a stdlib part into the document, returning its new part id. The part remains parametric — users can edit its params from the feature tree. Use `search_parts` first to discover the `path` and valid `params`. Applies an optional position to the part after insertion.",
        input_schema: {
          type: "object",
          properties: {
            path: {
              type: "string",
              description: "Part source path from `search_parts.id`, e.g. 'std:fastener.bolt.socket-head'.",
            },
            params: {
              type: "object",
              description: "Parameter name → value map. Missing params take the part's declared defaults.",
              additionalProperties: true,
            },
            position: {
              type: "object",
              description: "Optional world-space position {x, y, z} to place the part. Defaults to origin.",
              properties: {
                x: { type: "number" },
                y: { type: "number" },
                z: { type: "number" },
              },
            },
            name: {
              type: "string",
              description: "Optional display name override for the feature tree.",
            },
          },
          required: ["path"],
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

  /**
   * Build the full system prompt with type catalog and document state.
   * Delegates to `vcad_chat::build_system_prompt` via wasm when the
   * binding is wired, so the TS and the TUI produce byte-identical
   * prompts for the same inputs. Falls back to the inline TS string
   * below during bootstrap or in tests that don't mount wasm.
   */
  buildSystemPrompt(
    parts: Array<{ id: string; name: string; kind: string; nodes?: Array<{ nodeId: string; type: string; params: Record<string, unknown> }> }>,
    selection?: SelectionContext[],
  ): string {
    if (this.wasm) {
      try {
        return this.wasm.build_chat_system_prompt(
          JSON.stringify(parts),
          JSON.stringify(selection ?? []),
        );
      } catch {
        // Fall through to the TS fallback.
      }
    }
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

## Available Materials

Use set_material(part_id, material) to color parts. Preset keys:
- **Metals**: aluminum, steel, brass, copper, titanium, chrome, gold, silver
- **Plastics**: abs-white, abs-black, abs-red, abs-blue, pla, petg, nylon, resin, acrylic, rubber
- **Organic**: oak, walnut, leather, cork, bamboo
- **Glass**: glass, glass-tinted
- **Composite**: carbon-fiber, fiberglass, kevlar
- **Other**: concrete, ceramic, foam

For the Sun, use gold. For Mars, copper. For Earth, glass-tinted. For gold/silver metallic parts use gold/silver. There's no pure "color" system — you pick the closest preset.

## Orientation Notes

- **Cylinder**: axis along Z. Height is along Z, circular face is in XY plane. To make a wheel (round face visible from the side), use extrude with a circular sketch in the XZ plane (set y_dir to (0,0,1)) and direction along Y for thickness — this is more reliable than rotating a cylinder.
- **Box/Cube**: size.x is width, size.y is depth, size.z is height.
- The grid lies in the XY plane. Z is up. X is right, Y is forward.

## How translate/rotate/scale work (IMPORTANT)

When you call create with type translate/rotate/scale, it WRAPS the existing part in place. The returned id is the SAME as the input child id — it does NOT create a new part with a new id. So:
- create(type:translate, params:{child:abc, offset:...}) → part abc still exists, now translated. Use abc for follow-ups.
- The DAG node behind the part gets a new wrapping node, but you reference parts by their stable part id, not by node id.

## Key Rules

- Take as many tool calls as you need — there's no fixed cap. The user can click Stop to interrupt. Plan efficiently and stop naturally when the request is fulfilled; don't keep making changes for the sake of it.
- **CRITICAL — part IDs**: Every create tool returns a part id in the result (e.g. "Created cylinder with id: 1775951370705:0"). You MUST use the EXACT id string from the result in follow-up operations. NEVER invent ids like "part_1" or "part_2" — those will fail validation.
- **ALWAYS name features** with a short descriptive name param on every create call. Good names: "Front Wheel", "Top Tube", "Seat Post", "Left Eye". Bad names: (empty), "part1", "thing". Names make the feature tree readable and chat summaries meaningful.
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
~~~

A full circle of radius R using two half-arcs:
~~~
segments: [
  {type:"Arc", start:{x:R,y:0},  end:{x:-R,y:0}, center:{x:0,y:0}, ccw:false},
  {type:"Arc", start:{x:-R,y:0}, end:{x:R,y:0},  center:{x:0,y:0}, ccw:false}
]
~~~

## Sketch rules — READ CAREFULLY

- Sketches MUST have at least 2 segments forming a closed loop.
- Each segment's end must EXACTLY match the next segment's start.
- The LAST segment's end must match the FIRST segment's start.
- **Arcs**: start, end, center and ccw. The radius = distance(start, center) must equal distance(end, center) — if they don't match, the arc is invalid. For a full circle use TWO half-arcs (e.g. start=(r,0), end=(-r,0), center=(0,0), ccw=false, then start=(-r,0), end=(r,0), center=(0,0), ccw=false).
- To cut a shape OUT of a part, create the shape as a separate extrude, then use difference(left: mainPartId, right: cutPartId).
- Don't use single arcs — they aren't closed profiles.`,
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
        .map((s) => {
          // Surface sub-feature specifics so the AI can act on the exact
          // thing the user picked: a face index, an edge id, a vertex id.
          // Falls back to the part name + id for plain part selections.
          let detail = `${s.partName} (${s.geometryType}, id: ${s.partId})`;
          if (s.geometryType === "face" && s.faceIndex !== undefined) {
            detail = `${s.partName} face #${s.faceIndex} (id: ${s.partId})`;
          } else if (
            s.geometryType === "edge" &&
            s.dimensions?.edgeId !== undefined
          ) {
            detail = `${s.partName} edge #${s.dimensions.edgeId} (id: ${s.partId})`;
          } else if (
            s.geometryType === "vertex" &&
            s.dimensions?.vertexId !== undefined
          ) {
            detail = `${s.partName} vertex #${s.dimensions.vertexId} (id: ${s.partId})`;
          }
          return `- ${detail}`;
        })
        .join("\n");
      sections.push(`Selected:\n${selList}`);
    }

    return sections.join("\n");
  }
}

/** Singleton registry instance. */
export const commandRegistry = new CommandRegistry();
