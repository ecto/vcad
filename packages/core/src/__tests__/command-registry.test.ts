import { describe, it, expect, beforeEach } from "vitest";
import { CommandRegistry } from "../commands/registry.js";
import type { ToolSchemaEntry } from "../commands/types.js";
import { executeCrud } from "../commands/executors.js";

type MockPart = { id: string; name: string; kind: string };
function makeMockDocStore(parts: MockPart[] = []) {
  const partIndex = new Map(parts.map((p) => [p.id, p]));
  let nextId = 0;
  return {
    partIndex,
    parts,
    document: { nodes: {} as Record<string, unknown>, roots: [] },
    addPrimitive: (kind: string) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: kind, kind });
      parts.push({ id, name: kind, kind });
      return id;
    },
    updatePrimitiveOp: () => {},
    setTranslation: () => {},
    setRotation: () => {},
    setScale: () => {},
    setFeatureParam: () => {},
    setPartMaterial: () => {},
    applyBoolean: (type: string, _left: string, _right: string) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: type, kind: "boolean" });
      parts.push({ id, name: type, kind: "boolean" });
      return id;
    },
    addFillet: (_t: string, _r: number) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "fillet", kind: "fillet" });
      parts.push({ id, name: "fillet", kind: "fillet" });
      return id;
    },
    addChamfer: (_t: string, _d: number) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "chamfer", kind: "chamfer" });
      parts.push({ id, name: "chamfer", kind: "chamfer" });
      return id;
    },
    addShell: (_t: string, _t2: number) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "shell", kind: "shell" });
      parts.push({ id, name: "shell", kind: "shell" });
      return id;
    },
    addExtrude: (
      _plane: unknown,
      _origin: unknown,
      _segs: unknown[],
      _dir: unknown,
      _opts: unknown,
    ) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "extrude", kind: "extrude" });
      parts.push({ id, name: "extrude", kind: "extrude" });
      return id;
    },
    removePart: () => {},
  } as never;
}

function makeMockUiStore() {
  const selectedPartIds = new Set<string>();
  return {
    select: (id: string) => selectedPartIds.add(id),
    clearSelection: () => selectedPartIds.clear(),
    selectedPartIds,
  } as never;
}

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
    it("returns CRUD tools plus set_material and AI camera tools", () => {
      const tools = registry.toAnthropicTools();
      expect(tools.map((t) => t.name)).toEqual([
        "create",
        "read",
        "update",
        "delete",
        "set_material",
        "focus_part",
        "frame_all",
        "set_view",
        "tube",
        "polyline_tube",
        "inspect_part",
        "place",
        "describe_scene",
        "search_parts",
        "place_part",
      ]);
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

    it("set_material tool supports single-part, batch, and selector inputs", () => {
      const tools = registry.toAnthropicTools();
      const setMaterial = tools.find((t) => t.name === "set_material")!;
      const props = setMaterial.input_schema.properties as Record<string, unknown>;
      expect(props.part_id).toBeDefined();
      expect(props.part_ids).toBeDefined();
      expect(props.selector).toBeDefined();
      expect(props.material).toBeDefined();
      // Only `material` is required now — the target is chosen between
      // part_id / part_ids / selector at execution time.
      expect((setMaterial.input_schema.required as string[])).toEqual(["material"]);
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
      // 5 CRUD/material + 3 AI camera tools + 4 high-level tools
      // (tube, polyline_tube, inspect_part, place) + describe_scene
      // + 2 stdlib-parts tools (search_parts, place_part).
      expect(tools).toHaveLength(15);
      const create = tools[0]!;
      const typeEnum = (create.input_schema.properties as Record<string, Record<string, unknown>>)
        .type.enum as string[];
      expect(typeEnum).toEqual([]);
    });
  });

  describe("ExecutionResult display", () => {
    it("cube create returns summary with part link and size field", () => {
      const doc = makeMockDocStore();
      const ui = makeMockUiStore();
      const result = executeCrud(
        "create",
        { type: "cube", params: { size: { x: 50, y: 30, z: 10 } } },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      expect(result.display).toBeDefined();
      const summary = result.display!.summary;
      expect(summary.some((s) => s.type === "text" && s.text.includes("Cube"))).toBe(true);
      expect(summary.some((s) => s.type === "partLink")).toBe(true);
      expect(result.display!.fields).toContainEqual({ label: "size", value: "50×30×10 mm" });
      expect(result.display!.affectedPartIds).toHaveLength(1);
    });

    it("cylinder create returns summary with radius and height fields", () => {
      const doc = makeMockDocStore();
      const ui = makeMockUiStore();
      const result = executeCrud(
        "create",
        { type: "cylinder", params: { radius: 8, height: 20 } },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      expect(result.display!.fields).toContainEqual({ label: "radius", value: "8 mm" });
      expect(result.display!.fields).toContainEqual({ label: "height", value: "20 mm" });
    });

    it("translate returns summary with part link and offset field", () => {
      const doc = makeMockDocStore([{ id: "part-1", name: "Base", kind: "cube" }]);
      const ui = makeMockUiStore();
      const result = executeCrud(
        "create",
        { type: "translate", params: { child: "part-1", offset: { x: 10, y: 0, z: 0 } } },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      const segments = result.display!.summary;
      expect(segments.some((s) => s.type === "partLink" && s.partId === "part-1")).toBe(true);
      expect(result.display!.fields).toContainEqual({
        label: "offset",
        value: "(10, 0, 0) mm",
      });
    });

    it("difference returns summary with two input part links and result link", () => {
      const doc = makeMockDocStore([
        { id: "a", name: "Base", kind: "cube" },
        { id: "b", name: "Hole", kind: "cylinder" },
      ]);
      const ui = makeMockUiStore();
      const result = executeCrud(
        "create",
        { type: "difference", params: { left: "a", right: "b" } },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      const links = result.display!.summary.filter((s) => s.type === "partLink");
      expect(links).toHaveLength(3);
      expect(result.display!.affectedPartIds).toEqual(expect.arrayContaining(["a", "b"]));
    });

    it("fillet returns summary with target link and radius field", () => {
      const doc = makeMockDocStore([{ id: "p1", name: "Body", kind: "cube" }]);
      const ui = makeMockUiStore();
      const result = executeCrud(
        "create",
        { type: "fillet", params: { child: "p1", radius: 3 } },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      expect(result.display!.fields).toContainEqual({ label: "radius", value: "3 mm" });
      expect(result.display!.summary.some((s) => s.type === "partLink")).toBe(true);
    });

    it("extrude returns summary with segment count and depth field", () => {
      const doc = makeMockDocStore();
      const ui = makeMockUiStore();
      const result = executeCrud(
        "create",
        {
          type: "extrude",
          params: {
            sketch: {
              origin: { x: 0, y: 0, z: 0 },
              x_dir: { x: 1, y: 0, z: 0 },
              y_dir: { x: 0, y: 1, z: 0 },
              segments: [
                { type: "Line", start: { x: 0, y: 0 }, end: { x: 10, y: 0 } },
                { type: "Line", start: { x: 10, y: 0 }, end: { x: 10, y: 10 } },
                { type: "Line", start: { x: 10, y: 10 }, end: { x: 0, y: 10 } },
                { type: "Line", start: { x: 0, y: 10 }, end: { x: 0, y: 0 } },
              ],
            },
            direction: { x: 0, y: 0, z: 5 },
          },
        },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      expect(result.display!.fields).toContainEqual({ label: "segments", value: "4" });
      expect(result.display!.fields).toContainEqual({ label: "depth", value: "5.00 mm" });
    });

    it("delete returns summary with deleted part link", () => {
      const doc = makeMockDocStore([{ id: "p1", name: "Body", kind: "cube" }]);
      const ui = makeMockUiStore();
      const result = executeCrud("delete", { part_id: "p1" }, doc, ui);
      expect(result.status).toBe("success");
      expect(result.display!.summary.some((s) => s.type === "partLink" && s.partId === "p1")).toBe(true);
    });

    it("set_material returns summary with part link and material field", () => {
      const doc = makeMockDocStore([{ id: "p1", name: "Body", kind: "cube" }]);
      const ui = makeMockUiStore();
      const result = executeCrud(
        "set_material",
        { part_id: "p1", material: "aluminum" },
        doc,
        ui,
      );
      expect(result.status).toBe("success");
      expect(result.display!.fields).toContainEqual({ label: "material", value: "aluminum" });
    });

    it("executeCrud populates duration on all successful results", () => {
      const doc = makeMockDocStore();
      const ui = makeMockUiStore();
      const result = executeCrud("create", { type: "cube", params: {} }, doc, ui);
      expect(result.duration).toBeDefined();
      expect(typeof result.duration).toBe("number");
      expect(result.duration).toBeGreaterThanOrEqual(0);
    });

    it("error results have no display field", () => {
      const doc = makeMockDocStore();
      const ui = makeMockUiStore();
      const result = executeCrud("create", { type: "cone", params: {} }, doc, ui);
      expect(result.status).toBe("error");
      expect(result.display).toBeUndefined();
    });
  });
});
