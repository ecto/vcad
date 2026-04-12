import { describe, it, expect, beforeEach } from "vitest";
import { CommandRegistry } from "../commands/registry.js";
import type { ToolSchemaEntry } from "../commands/types.js";

// Minimal fixture matching what the Rust proc macro generates (serde snake_case).
const SAMPLE_SCHEMAS: ToolSchemaEntry[] = [
  {
    name: "cube",
    description: "Axis-aligned box centered at origin.",
    category: "primitive",
    ai_hint: "Use for rectangular/box shapes.",
    input_schema: {
      type: "object",
      properties: {
        size: {
          type: "object",
          properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } },
          required: ["x", "y", "z"],
          description: "Size along each axis.",
        },
      },
      required: ["size"],
    },
  },
  {
    name: "cylinder",
    description: "Cylinder along the Z axis, centered at origin.",
    category: "primitive",
    input_schema: {
      type: "object",
      properties: {
        radius: { type: "number", description: "Radius of the cylinder." },
        height: { type: "number", description: "Height of the cylinder." },
        segments: { type: "integer", description: "Number of circular segments (0 = auto)." },
      },
      required: ["radius", "height", "segments"],
    },
  },
  {
    name: "extrude",
    description: "Extrude a sketch profile along a direction vector.",
    category: "sketch_op",
    ai_hint: "Extrude a sketch profile into 3D.",
    input_schema: {
      type: "object",
      properties: {
        sketch: { type: "string", description: "Node ID reference" },
        direction: {
          type: "object",
          properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } },
          required: ["x", "y", "z"],
          description: "Extrusion direction and distance.",
        },
        twist_angle: { type: "number", description: "Optional twist angle in radians." },
      },
      required: ["sketch", "direction"],
    },
  },
  {
    name: "fillet",
    description: "Fillet — round edges of a solid.",
    category: "modifier",
    input_schema: {
      type: "object",
      properties: {
        child: { type: "string", description: "Node ID reference" },
        radius: { type: "number", description: "Fillet radius." },
      },
      required: ["child", "radius"],
    },
  },
];

describe("CommandRegistry", () => {
  let registry: CommandRegistry;

  beforeEach(() => {
    registry = new CommandRegistry();
    registry.loadSchemas(JSON.stringify(SAMPLE_SCHEMAS));
  });

  describe("loadSchemas", () => {
    it("loads schemas from JSON string", () => {
      expect(registry.getSchemas()).toHaveLength(4);
    });

    it("parses snake_case field names correctly", () => {
      const cube = registry.getSchemas()[0]!;
      expect(cube.input_schema).toBeDefined();
      expect(cube.ai_hint).toBe("Use for rectangular/box shapes.");
    });

    it("handles missing optional fields", () => {
      const cylinder = registry.getSchemas()[1]!;
      expect(cylinder.ai_hint).toBeUndefined();
    });
  });

  describe("getTypeEnum", () => {
    it("returns all type names", () => {
      expect(registry.getTypeEnum()).toEqual(["cube", "cylinder", "extrude", "fillet"]);
    });
  });

  describe("toAnthropicTools", () => {
    it("returns CRUD tools plus set_material", () => {
      const tools = registry.toAnthropicTools();
      expect(tools).toHaveLength(5);
      expect(tools.map((t) => t.name)).toEqual(["create", "read", "update", "delete", "set_material"]);
    });

    it("create tool has type enum from schemas", () => {
      const tools = registry.toAnthropicTools();
      const create = tools[0]!;
      const typeEnum = (create.input_schema.properties as Record<string, Record<string, unknown>>)
        .type.enum as string[];
      expect(typeEnum).toEqual(["cube", "cylinder", "extrude", "fillet"]);
    });

    it("all tools have input_schema with type: object", () => {
      for (const tool of registry.toAnthropicTools()) {
        expect(tool.input_schema.type).toBe("object");
        expect(tool.input_schema.properties).toBeDefined();
      }
    });

    it("set_material tool has part_id and material params", () => {
      const tools = registry.toAnthropicTools();
      const setMaterial = tools.find((t) => t.name === "set_material")!;
      const props = setMaterial.input_schema.properties as Record<string, unknown>;
      expect(props.part_id).toBeDefined();
      expect(props.material).toBeDefined();
      expect((setMaterial.input_schema.required as string[])).toEqual(["part_id", "material"]);
    });
  });

  describe("getTypeCatalog", () => {
    it("includes all type names", () => {
      const catalog = registry.getTypeCatalog();
      expect(catalog).toContain("### cube (primitive)");
      expect(catalog).toContain("### cylinder (primitive)");
      expect(catalog).toContain("### extrude (sketch_op)");
      expect(catalog).toContain("### fillet (modifier)");
    });

    it("includes descriptions", () => {
      const catalog = registry.getTypeCatalog();
      expect(catalog).toContain("Axis-aligned box centered at origin.");
    });

    it("includes ai_hint when present", () => {
      const catalog = registry.getTypeCatalog();
      expect(catalog).toContain("Use for rectangular/box shapes.");
    });

    it("includes parameter listings", () => {
      const catalog = registry.getTypeCatalog();
      expect(catalog).toContain("- size: object");
      expect(catalog).toContain("- radius: number");
    });

    it("caches result", () => {
      const first = registry.getTypeCatalog();
      const second = registry.getTypeCatalog();
      expect(first).toBe(second); // same reference = cached
    });

    it("invalidates cache on reload", () => {
      const first = registry.getTypeCatalog();
      registry.loadSchemas(JSON.stringify([SAMPLE_SCHEMAS[0]]));
      const second = registry.getTypeCatalog();
      expect(second).not.toBe(first);
      expect(second).not.toContain("cylinder");
    });
  });

  describe("buildSystemPrompt", () => {
    it("includes base instructions", () => {
      const prompt = registry.buildSystemPrompt([]);
      expect(prompt).toContain("parametric CAD copilot");
      expect(prompt).toContain("Z-up");
      expect(prompt).toContain("create, read, update, delete");
    });

    it("includes type catalog", () => {
      const prompt = registry.buildSystemPrompt([]);
      expect(prompt).toContain("## Type Catalog");
      expect(prompt).toContain("### cube (primitive)");
    });

    it("includes document parts when provided", () => {
      const parts = [
        { id: "part-1", name: "Base Plate", kind: "cube" },
        { id: "part-2", name: "Pin", kind: "cylinder" },
      ];
      const prompt = registry.buildSystemPrompt(parts);
      expect(prompt).toContain("## Current Document");
      expect(prompt).toContain('part-1 "Base Plate" [cube]');
      expect(prompt).toContain('part-2 "Pin" [cylinder]');
    });

    it("includes selection context when provided", () => {
      const selection = [
        { partId: "part-1", partName: "Base Plate", geometryType: "part" as const },
      ];
      const prompt = registry.buildSystemPrompt([], selection);
      expect(prompt).toContain("Selected:");
      expect(prompt).toContain("Base Plate (part, id: part-1)");
    });

    it("omits document section when no parts", () => {
      const prompt = registry.buildSystemPrompt([]);
      expect(prompt).not.toContain("## Current Document");
    });
  });

  describe("default static schemas", () => {
    it("loads static schemas at construction time", () => {
      const fresh = new CommandRegistry();
      // Static schemas from CsgOp (21 non-hidden variants)
      expect(fresh.getSchemas().length).toBeGreaterThan(0);
      expect(fresh.getTypeEnum()).toContain("cube");
      expect(fresh.getTypeEnum()).toContain("extrude");
      expect(fresh.getTypeEnum()).toContain("fillet");
    });

    it("can be overridden by loadSchemas", () => {
      const fresh = new CommandRegistry();
      fresh.loadSchemas("[]");
      const tools = fresh.toAnthropicTools();
      expect(tools).toHaveLength(5);
      const create = tools[0]!;
      const typeEnum = (create.input_schema.properties as Record<string, Record<string, unknown>>)
        .type.enum as string[];
      expect(typeEnum).toEqual([]);
    });
  });
});
