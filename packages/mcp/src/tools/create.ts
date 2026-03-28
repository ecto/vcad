/**
 * create_cad_document tool — build geometry from structured input.
 */

import type { Document, Node, NodeId, CsgOp, Vec3, Vec2, SketchSegment2D } from "@vcad/ir";
import { createDocument, toVCode } from "@vcad/ir";

/** Primitive definition for tool input. */
interface Primitive {
  type: "cube" | "cylinder" | "sphere" | "cone";
  // Cube
  size?: Vec3;
  // Cylinder/Sphere/Cone
  radius?: number;
  height?: number;
  segments?: number;
  // Cone
  radius_bottom?: number;
  radius_top?: number;
}

/** Named position for relative positioning. */
type NamedPosition = "center" | "top-center" | "bottom-center";

/** Coordinate value: absolute number or percentage string. */
type CoordinateValue = number | string;

/** Position specification: absolute Vec3, named position, or percentage-based. */
type PositionSpec =
  | Vec3
  | NamedPosition
  | { x: CoordinateValue; y: CoordinateValue; z?: CoordinateValue };

/** Bounding box representation. */
interface BBox {
  min: Vec3;
  max: Vec3;
}

// ============================================================================
// Sketch Input Types (for AI-friendly sketch-based operations)
// ============================================================================

/** Rectangle shape for sketch input. */
interface RectangleShape {
  type: "rectangle";
  width: number;
  height: number;
  centered?: boolean;  // If true, centered at origin; if false, corner at origin
}

/** Circle shape for sketch input. */
interface CircleShape {
  type: "circle";
  radius: number;
}

/** Polygon shape for sketch input. */
interface PolygonShape {
  type: "polygon";
  points: Array<{ x: number; y: number }>;
  closed?: boolean;  // Default true
}

/** High-level sketch input format for AI usability. */
interface SketchInput {
  plane?: "xy" | "xz" | "yz";  // Default: xy
  at?: Vec3;                   // Sketch plane origin (default: 0,0,0)
  shape: RectangleShape | CircleShape | PolygonShape;
}

/** Path for sweep operations - line segment. */
interface LinePathInput {
  type: "line";
  start: Vec3;
  end: Vec3;
}

/** Path for sweep operations - helix. */
interface HelixPathInput {
  type: "helix";
  radius: number;
  pitch: number;
  height: number;
}

/** Path input for sweep operations. */
type PathInput = LinePathInput | HelixPathInput;

/** Operation to apply to geometry. */
interface Operation {
  type:
    | "union"
    | "difference"
    | "intersection"
    | "translate"
    | "rotate"
    | "scale"
    | "linear_pattern"
    | "circular_pattern"
    | "hole"
    | "fillet"
    | "chamfer"
    | "shell"
    | "extrude"
    | "revolve"
    | "sweep"
    | "loft";
  // For boolean ops
  primitive?: Primitive;
  at?: PositionSpec;
  // For hole
  diameter?: number;
  depth?: number;
  // For translate
  offset?: Vec3;
  // For rotate (degrees)
  angles?: Vec3;
  // For scale
  factor?: Vec3;
  // For linear_pattern
  direction?: Vec3;
  count?: number;
  spacing?: number;
  // For circular_pattern
  axis_origin?: Vec3;
  axis_dir?: Vec3;
  angle_deg?: number;
  // For fillet
  radius?: number;
  // For chamfer
  distance?: number;
  // For shell
  thickness?: number;
  // For extrude
  sketch?: SketchInput;
  height?: number;
  // For revolve
  axis?: "x" | "y" | "z" | Vec3;
  axis_offset?: number;
  // For sweep
  path?: PathInput;
  twist_deg?: number;
  scale_start?: number;
  scale_end?: number;
  // For loft
  sketches?: SketchInput[];
  closed?: boolean;
}

/** Part definition for tool input (primitive-based). */
interface PrimitivePartInput {
  name: string;
  primitive: Primitive;
  operations?: Operation[];
  material?: string;
}

/** Part definition for tool input (sketch-based extrude). */
interface ExtrudePartInput {
  name: string;
  extrude: {
    sketch: SketchInput;
    height?: number;
    direction?: Vec3;
  };
  operations?: Operation[];
  material?: string;
}

/** Part definition for tool input (sketch-based revolve). */
interface RevolvePartInput {
  name: string;
  revolve: {
    sketch: SketchInput;
    axis?: "x" | "y" | "z" | Vec3;
    axis_offset?: number;
    angle_deg?: number;
  };
  operations?: Operation[];
  material?: string;
}

/** Part definition for tool input (sketch-based sweep). */
interface SweepPartInput {
  name: string;
  sweep: {
    sketch: SketchInput;
    path: PathInput;
    twist_deg?: number;
    scale_start?: number;
    scale_end?: number;
  };
  operations?: Operation[];
  material?: string;
}

/** Part definition for tool input (sketch-based loft). */
interface LoftPartInput {
  name: string;
  loft: {
    sketches: SketchInput[];
    closed?: boolean;
  };
  operations?: Operation[];
  material?: string;
}

/** Part definition for tool input. */
type PartInput = PrimitivePartInput | ExtrudePartInput | RevolvePartInput | SweepPartInput | LoftPartInput;

// ============================================================================
// Assembly Types (for physics simulation and robotics)
// ============================================================================

/** Instance of a part in an assembly. */
interface InstanceInput {
  id: string;
  part: string;           // References part by name
  name?: string;
  position?: Vec3;
  rotation?: Vec3;        // Euler degrees
}

/** Joint connecting two instances. */
interface JointInput {
  id: string;
  name?: string;
  parent: string | null;  // Instance ID or null for ground
  child: string;          // Instance ID
  type: "fixed" | "revolute" | "slider" | "cylindrical" | "ball";
  axis?: "x" | "y" | "z" | Vec3;
  parent_anchor?: Vec3;
  child_anchor?: Vec3;
  limits?: [number, number];
  state?: number;
}

/** Assembly definition for physics simulation. */
interface AssemblyInput {
  instances: InstanceInput[];
  joints: JointInput[];
  ground?: string;        // Instance ID of fixed part
}

/** Input schema for create_cad_document. */
interface CreateInput {
  parts: PartInput[];
  assembly?: AssemblyInput;
  format?: "json" | "vcode";
}

/** Compute bounding box from a primitive definition. */
function getPrimitiveBBox(prim: Primitive): BBox {
  switch (prim.type) {
    case "cube": {
      const size = prim.size ?? { x: 10, y: 10, z: 10 };
      // Cube: corner at (0,0,0), extends to (size.x, size.y, size.z)
      return {
        min: { x: 0, y: 0, z: 0 },
        max: { x: size.x, y: size.y, z: size.z },
      };
    }
    case "cylinder": {
      const r = prim.radius ?? 5;
      const h = prim.height ?? 10;
      // Cylinder: base center at (0,0,0), height along +Z
      return {
        min: { x: -r, y: -r, z: 0 },
        max: { x: r, y: r, z: h },
      };
    }
    case "sphere": {
      const r = prim.radius ?? 5;
      // Sphere: center at (0,0,0)
      return {
        min: { x: -r, y: -r, z: -r },
        max: { x: r, y: r, z: r },
      };
    }
    case "cone": {
      const rb = prim.radius_bottom ?? prim.radius ?? 5;
      const rt = prim.radius_top ?? 0;
      const h = prim.height ?? 10;
      const maxR = Math.max(rb, rt);
      // Cone: base center at (0,0,0), height along +Z
      return {
        min: { x: -maxR, y: -maxR, z: 0 },
        max: { x: maxR, y: maxR, z: h },
      };
    }
  }
}

/** Resolve a single coordinate value (number or percentage string). */
function resolveCoordinate(
  value: CoordinateValue,
  minVal: number,
  maxVal: number,
): number {
  if (typeof value === "number") {
    return value;
  }
  // Parse percentage string like "50%"
  const match = value.match(/^(-?\d+(?:\.\d+)?)\s*%$/);
  if (match) {
    const pct = parseFloat(match[1]) / 100;
    return minVal + pct * (maxVal - minVal);
  }
  throw new Error(`Invalid coordinate value: ${value}`);
}

/**
 * Convert a high-level SketchInput shape to SketchSegment2D array.
 * This makes it easy for AI agents to create sketch-based geometry without
 * knowing the low-level segment format.
 */
function convertSketchToSegments(sketch: SketchInput): SketchSegment2D[] {
  const { shape } = sketch;

  switch (shape.type) {
    case "rectangle": {
      const { width, height, centered } = shape;
      const x0 = centered ? -width / 2 : 0;
      const y0 = centered ? -height / 2 : 0;
      const x1 = centered ? width / 2 : width;
      const y1 = centered ? height / 2 : height;

      // Create 4 line segments forming a closed rectangle
      return [
        { type: "Line", start: { x: x0, y: y0 }, end: { x: x1, y: y0 } },
        { type: "Line", start: { x: x1, y: y0 }, end: { x: x1, y: y1 } },
        { type: "Line", start: { x: x1, y: y1 }, end: { x: x0, y: y1 } },
        { type: "Line", start: { x: x0, y: y1 }, end: { x: x0, y: y0 } },
      ];
    }

    case "circle": {
      const { radius } = shape;
      // Approximate circle with 4 arcs (quarter circles)
      // Each arc spans 90 degrees
      return [
        {
          type: "Arc",
          start: { x: radius, y: 0 },
          end: { x: 0, y: radius },
          center: { x: 0, y: 0 },
          ccw: true,
        },
        {
          type: "Arc",
          start: { x: 0, y: radius },
          end: { x: -radius, y: 0 },
          center: { x: 0, y: 0 },
          ccw: true,
        },
        {
          type: "Arc",
          start: { x: -radius, y: 0 },
          end: { x: 0, y: -radius },
          center: { x: 0, y: 0 },
          ccw: true,
        },
        {
          type: "Arc",
          start: { x: 0, y: -radius },
          end: { x: radius, y: 0 },
          center: { x: 0, y: 0 },
          ccw: true,
        },
      ];
    }

    case "polygon": {
      const { points, closed = true } = shape;
      if (points.length < 2) {
        throw new Error("Polygon requires at least 2 points");
      }

      const segments: SketchSegment2D[] = [];
      for (let i = 0; i < points.length - 1; i++) {
        segments.push({
          type: "Line",
          start: { x: points[i].x, y: points[i].y },
          end: { x: points[i + 1].x, y: points[i + 1].y },
        });
      }

      // Close the polygon if requested
      if (closed && points.length >= 3) {
        const last = points[points.length - 1];
        const first = points[0];
        segments.push({
          type: "Line",
          start: { x: last.x, y: last.y },
          end: { x: first.x, y: first.y },
        });
      }

      return segments;
    }
  }
}

/**
 * Get sketch plane vectors from plane specification.
 * Returns [origin, x_dir, y_dir] for the sketch plane.
 */
function getSketchPlaneVectors(
  sketch: SketchInput,
): { origin: Vec3; x_dir: Vec3; y_dir: Vec3 } {
  const plane = sketch.plane ?? "xy";
  const origin = sketch.at ?? { x: 0, y: 0, z: 0 };

  switch (plane) {
    case "xy":
      return {
        origin,
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 1, z: 0 },
      };
    case "xz":
      return {
        origin,
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 0, z: 1 },
      };
    case "yz":
      return {
        origin,
        x_dir: { x: 0, y: 1, z: 0 },
        y_dir: { x: 0, y: 0, z: 1 },
      };
  }
}

/**
 * Create a Sketch2D node from a SketchInput and return its ID.
 */
function createSketchNode(
  sketch: SketchInput,
  nodes: Record<string, Node>,
  nextId: { value: NodeId },
): NodeId {
  const id = nextId.value++;
  const { origin, x_dir, y_dir } = getSketchPlaneVectors(sketch);
  const segments = convertSketchToSegments(sketch);

  nodes[String(id)] = {
    id,
    name: null,
    op: {
      type: "Sketch2D",
      origin,
      x_dir,
      y_dir,
      segments,
    },
  };

  return id;
}

/** Resolve a position specification to absolute Vec3 coordinates. */
function resolvePosition(pos: PositionSpec, basePrim: Primitive): Vec3 {
  const bbox = getPrimitiveBBox(basePrim);

  // Named positions
  if (typeof pos === "string") {
    const centerX = (bbox.min.x + bbox.max.x) / 2;
    const centerY = (bbox.min.y + bbox.max.y) / 2;
    const centerZ = (bbox.min.z + bbox.max.z) / 2;

    switch (pos) {
      case "center":
        return { x: centerX, y: centerY, z: centerZ };
      case "top-center":
        return { x: centerX, y: centerY, z: bbox.max.z };
      case "bottom-center":
        return { x: centerX, y: centerY, z: bbox.min.z };
      default:
        throw new Error(`Unknown named position: ${pos}`);
    }
  }

  // Object with coordinates (absolute or percentage)
  return {
    x: resolveCoordinate(pos.x, bbox.min.x, bbox.max.x),
    y: resolveCoordinate(pos.y, bbox.min.y, bbox.max.y),
    z: resolveCoordinate(pos.z ?? 0, bbox.min.z, bbox.max.z),
  };
}

/** JSON Schema for input validation. */
export const createCadDocumentSchema = {
  type: "object" as const,
  properties: {
    format: {
      type: "string" as const,
      enum: ["json", "vcode"],
      description: "Output format: 'json' (verbose, human-readable) or 'compact' (token-efficient, ~5x smaller). Default: 'compact'",
    },
    parts: {
      type: "array" as const,
      description: "Parts to create in the document",
      items: {
        type: "object" as const,
        properties: {
          name: {
            type: "string" as const,
            description: "Name of the part",
          },
          primitive: {
            type: "object" as const,
            description:
              "Base primitive shape. Origins: cube has corner at (0,0,0) extending to (size.x, size.y, size.z); cylinder has base center at (0,0,0) with height along +Z; sphere has center at (0,0,0); cone has base center at (0,0,0) with height along +Z.",
            properties: {
              type: {
                type: "string" as const,
                enum: ["cube", "cylinder", "sphere", "cone"],
                description:
                  "Primitive type. Cube: corner at origin. Cylinder: base center at origin, +Z up. Sphere: center at origin. Cone: base center at origin, +Z up.",
              },
              size: {
                type: "object" as const,
                description:
                  "Size for cube (x, y, z dimensions in mm). Cube extends from (0,0,0) to (size.x, size.y, size.z).",
                properties: {
                  x: { type: "number" as const },
                  y: { type: "number" as const },
                  z: { type: "number" as const },
                },
              },
              radius: {
                type: "number" as const,
                description: "Radius for cylinder/sphere (mm)",
              },
              height: {
                type: "number" as const,
                description: "Height for cylinder/cone (mm), extends along +Z axis",
              },
              segments: {
                type: "number" as const,
                description: "Number of segments for curved surfaces (default: 32)",
              },
              radius_bottom: {
                type: "number" as const,
                description: "Bottom radius for cone (mm)",
              },
              radius_top: {
                type: "number" as const,
                description: "Top radius for cone (mm, 0 for pointed cone)",
              },
            },
            required: ["type"],
          },
          operations: {
            type: "array" as const,
            description: "Operations to apply (in order)",
            items: {
              type: "object" as const,
              properties: {
                type: {
                  type: "string" as const,
                  enum: [
                    "union",
                    "difference",
                    "intersection",
                    "translate",
                    "rotate",
                    "scale",
                    "linear_pattern",
                    "circular_pattern",
                    "hole",
                    "fillet",
                    "chamfer",
                    "shell",
                  ],
                  description:
                    "Operation type. 'hole' creates a vertical through-hole. 'fillet' rounds edges. 'chamfer' bevels edges. 'shell' hollows the part.",
                },
                primitive: {
                  type: "object" as const,
                  description: "Primitive for boolean operations",
                },
                at: {
                  oneOf: [
                    {
                      type: "object" as const,
                      description: "Absolute position {x, y, z} in mm",
                      properties: {
                        x: { type: "number" as const },
                        y: { type: "number" as const },
                        z: { type: "number" as const },
                      },
                    },
                    {
                      type: "string" as const,
                      enum: ["center", "top-center", "bottom-center"],
                      description: "Named position relative to base primitive",
                    },
                    {
                      type: "object" as const,
                      description:
                        "Percentage position {x: '50%', y: '50%'} relative to base primitive bounds",
                      properties: {
                        x: { oneOf: [{ type: "number" as const }, { type: "string" as const }] },
                        y: { oneOf: [{ type: "number" as const }, { type: "string" as const }] },
                        z: { oneOf: [{ type: "number" as const }, { type: "string" as const }] },
                      },
                    },
                  ],
                  description:
                    "Position for operation. Accepts: absolute {x,y,z}, named ('center', 'top-center', 'bottom-center'), or percentage ({x:'50%', y:'50%'}).",
                },
                diameter: {
                  type: "number" as const,
                  description: "Diameter for hole operation (mm)",
                },
                depth: {
                  type: "number" as const,
                  description:
                    "Depth for hole operation (mm). Omit for through-hole (auto-sized to pass through part).",
                },
                offset: {
                  type: "object" as const,
                  description: "Translation offset (mm)",
                  properties: {
                    x: { type: "number" as const },
                    y: { type: "number" as const },
                    z: { type: "number" as const },
                  },
                },
                angles: {
                  type: "object" as const,
                  description: "Rotation angles (degrees)",
                  properties: {
                    x: { type: "number" as const },
                    y: { type: "number" as const },
                    z: { type: "number" as const },
                  },
                },
                factor: {
                  type: "object" as const,
                  description: "Scale factors",
                  properties: {
                    x: { type: "number" as const },
                    y: { type: "number" as const },
                    z: { type: "number" as const },
                  },
                },
                direction: {
                  type: "object" as const,
                  description: "Direction for linear pattern",
                  properties: {
                    x: { type: "number" as const },
                    y: { type: "number" as const },
                    z: { type: "number" as const },
                  },
                },
                count: {
                  type: "number" as const,
                  description: "Number of copies for patterns",
                },
                spacing: {
                  type: "number" as const,
                  description: "Spacing for linear pattern (mm)",
                },
                axis_origin: {
                  type: "object" as const,
                  description: "Axis origin for circular pattern",
                  properties: {
                    x: { type: "number" as const },
                    y: { type: "number" as const },
                    z: { type: "number" as const },
                  },
                },
                axis_dir: {
                  type: "object" as const,
                  description: "Axis direction for circular pattern",
                  properties: {
                    x: { type: "number" as const },
                    y: { type: "number" as const },
                    z: { type: "number" as const },
                  },
                },
                angle_deg: {
                  type: "number" as const,
                  description: "Total angle for circular pattern (degrees)",
                },
                radius: {
                  type: "number" as const,
                  description: "Radius for fillet operation (mm)",
                },
                distance: {
                  type: "number" as const,
                  description: "Distance for chamfer operation (mm)",
                },
                thickness: {
                  type: "number" as const,
                  description: "Wall thickness for shell operation (mm)",
                },
              },
              required: ["type"],
            },
          },
          material: {
            type: "string" as const,
            description: "Material key (e.g., 'aluminum', 'steel')",
          },
        },
        required: ["name"],
      },
    },
    assembly: {
      type: "object" as const,
      description: "Optional assembly definition for physics simulation. Define instances of parts and joints connecting them.",
      properties: {
        instances: {
          type: "array" as const,
          description: "Part instances in the assembly",
          items: {
            type: "object" as const,
            properties: {
              id: { type: "string" as const, description: "Unique instance ID" },
              part: { type: "string" as const, description: "Part name to instantiate" },
              name: { type: "string" as const, description: "Display name (optional)" },
              position: {
                type: "object" as const,
                description: "Initial position {x, y, z} in mm",
                properties: {
                  x: { type: "number" as const },
                  y: { type: "number" as const },
                  z: { type: "number" as const },
                },
              },
              rotation: {
                type: "object" as const,
                description: "Initial rotation {x, y, z} in degrees",
                properties: {
                  x: { type: "number" as const },
                  y: { type: "number" as const },
                  z: { type: "number" as const },
                },
              },
            },
            required: ["id", "part"],
          },
        },
        joints: {
          type: "array" as const,
          description: "Joints connecting instances",
          items: {
            type: "object" as const,
            properties: {
              id: { type: "string" as const, description: "Unique joint ID" },
              name: { type: "string" as const, description: "Display name (optional)" },
              parent: {
                oneOf: [{ type: "string" as const }, { type: "null" as const }],
                description: "Parent instance ID, or null for ground/world",
              },
              child: { type: "string" as const, description: "Child instance ID" },
              type: {
                type: "string" as const,
                enum: ["fixed", "revolute", "slider", "cylindrical", "ball"],
                description: "Joint type: fixed, revolute (hinge), slider (prismatic), cylindrical, or ball",
              },
              axis: {
                oneOf: [
                  { type: "string" as const, enum: ["x", "y", "z"] },
                  {
                    type: "object" as const,
                    properties: {
                      x: { type: "number" as const },
                      y: { type: "number" as const },
                      z: { type: "number" as const },
                    },
                  },
                ],
                description: "Joint axis: 'x', 'y', 'z', or custom {x, y, z} vector",
              },
              parent_anchor: {
                type: "object" as const,
                description: "Anchor point on parent in parent's local coordinates",
                properties: {
                  x: { type: "number" as const },
                  y: { type: "number" as const },
                  z: { type: "number" as const },
                },
              },
              child_anchor: {
                type: "object" as const,
                description: "Anchor point on child in child's local coordinates",
                properties: {
                  x: { type: "number" as const },
                  y: { type: "number" as const },
                  z: { type: "number" as const },
                },
              },
              limits: {
                type: "array" as const,
                items: { type: "number" as const },
                minItems: 2,
                maxItems: 2,
                description: "Joint limits [min, max] in degrees for revolute, mm for slider",
              },
              state: {
                type: "number" as const,
                description: "Initial joint state (angle in degrees or position in mm)",
              },
            },
            required: ["id", "parent", "child", "type"],
          },
        },
        ground: {
          type: "string" as const,
          description: "Instance ID of the fixed/ground part",
        },
      },
      required: ["instances", "joints"],
    },
  },
  required: ["parts"],
};

// Sketch input schema for sketch-based operations
const sketchInputSchema = {
  type: "object" as const,
  description: "Sketch definition for extrude/revolve/sweep/loft operations",
  properties: {
    plane: {
      type: "string" as const,
      enum: ["xy", "xz", "yz"],
      description: "Sketch plane (default: xy)",
    },
    at: {
      type: "object" as const,
      description: "Sketch plane origin (default: 0,0,0)",
      properties: {
        x: { type: "number" as const },
        y: { type: "number" as const },
        z: { type: "number" as const },
      },
    },
    shape: {
      oneOf: [
        {
          type: "object" as const,
          description: "Rectangle shape",
          properties: {
            type: { const: "rectangle" as const },
            width: { type: "number" as const, description: "Width in mm" },
            height: { type: "number" as const, description: "Height in mm" },
            centered: { type: "boolean" as const, description: "Center at origin (default: false)" },
          },
          required: ["type", "width", "height"],
        },
        {
          type: "object" as const,
          description: "Circle shape",
          properties: {
            type: { const: "circle" as const },
            radius: { type: "number" as const, description: "Radius in mm" },
          },
          required: ["type", "radius"],
        },
        {
          type: "object" as const,
          description: "Polygon shape",
          properties: {
            type: { const: "polygon" as const },
            points: {
              type: "array" as const,
              items: {
                type: "object" as const,
                properties: {
                  x: { type: "number" as const },
                  y: { type: "number" as const },
                },
                required: ["x", "y"],
              },
              description: "Array of 2D points",
            },
            closed: { type: "boolean" as const, description: "Close the polygon (default: true)" },
          },
          required: ["type", "points"],
        },
      ],
    },
  },
  required: ["shape"],
};

/** Create a primitive node and return its ID. */
function createPrimitiveNode(
  prim: Primitive,
  nodes: Record<string, Node>,
  nextId: { value: NodeId },
): NodeId {
  const id = nextId.value++;
  let op: CsgOp;

  switch (prim.type) {
    case "cube":
      op = {
        type: "Cube",
        size: prim.size ?? { x: 10, y: 10, z: 10 },
      };
      break;
    case "cylinder":
      op = {
        type: "Cylinder",
        radius: prim.radius ?? 5,
        height: prim.height ?? 10,
        segments: prim.segments ?? 32,
      };
      break;
    case "sphere":
      op = {
        type: "Sphere",
        radius: prim.radius ?? 5,
        segments: prim.segments ?? 32,
      };
      break;
    case "cone":
      op = {
        type: "Cone",
        radius_bottom: prim.radius_bottom ?? prim.radius ?? 5,
        radius_top: prim.radius_top ?? 0,
        height: prim.height ?? 10,
        segments: prim.segments ?? 32,
      };
      break;
  }

  nodes[String(id)] = { id, name: null, op };
  return id;
}

/**
 * Type guard to check if a part is primitive-based.
 */
function isPrimitivePart(part: PartInput): part is PrimitivePartInput {
  return "primitive" in part;
}

/**
 * Type guard to check if a part is extrude-based.
 */
function isExtrudePart(part: PartInput): part is ExtrudePartInput {
  return "extrude" in part;
}

/**
 * Type guard to check if a part is revolve-based.
 */
function isRevolvePart(part: PartInput): part is RevolvePartInput {
  return "revolve" in part;
}

/**
 * Type guard to check if a part is sweep-based.
 */
function isSweepPart(part: PartInput): part is SweepPartInput {
  return "sweep" in part;
}

/**
 * Type guard to check if a part is loft-based.
 */
function isLoftPart(part: PartInput): part is LoftPartInput {
  return "loft" in part;
}

/**
 * Create the base geometry node for a part (primitive, extrude, revolve, sweep, or loft).
 */
function createPartBaseNode(
  part: PartInput,
  nodes: Record<string, Node>,
  nextId: { value: NodeId },
): NodeId {
  if (isPrimitivePart(part)) {
    return createPrimitiveNode(part.primitive, nodes, nextId);
  }

  if (isExtrudePart(part)) {
    const { sketch, height, direction } = part.extrude;
    const sketchId = createSketchNode(sketch, nodes, nextId);

    // Determine extrusion direction
    const plane = sketch.plane ?? "xy";
    let extrudeDir: Vec3;
    if (direction) {
      extrudeDir = direction;
    } else {
      // Default: extrude normal to sketch plane
      const h = height ?? 10;
      switch (plane) {
        case "xy":
          extrudeDir = { x: 0, y: 0, z: h };
          break;
        case "xz":
          extrudeDir = { x: 0, y: h, z: 0 };
          break;
        case "yz":
          extrudeDir = { x: h, y: 0, z: 0 };
          break;
      }
    }

    const extrudeId = nextId.value++;
    nodes[String(extrudeId)] = {
      id: extrudeId,
      name: null,
      op: {
        type: "Extrude",
        sketch: sketchId,
        direction: extrudeDir,
      },
    };

    return extrudeId;
  }

  if (isRevolvePart(part)) {
    const { sketch, axis, axis_offset, angle_deg } = part.revolve;
    const sketchId = createSketchNode(sketch, nodes, nextId);

    // Determine axis origin and direction
    let axisOrigin: Vec3;
    let axisDir: Vec3;
    const offset = axis_offset ?? 0;

    if (typeof axis === "string" || axis === undefined) {
      const axisName = axis ?? "z";
      switch (axisName) {
        case "x":
          axisOrigin = { x: 0, y: offset, z: 0 };
          axisDir = { x: 1, y: 0, z: 0 };
          break;
        case "y":
          axisOrigin = { x: offset, y: 0, z: 0 };
          axisDir = { x: 0, y: 1, z: 0 };
          break;
        case "z":
          axisOrigin = { x: offset, y: 0, z: 0 };
          axisDir = { x: 0, y: 0, z: 1 };
          break;
      }
    } else {
      // Custom axis direction
      axisOrigin = { x: offset, y: 0, z: 0 };
      axisDir = axis;
    }

    const revolveId = nextId.value++;
    nodes[String(revolveId)] = {
      id: revolveId,
      name: null,
      op: {
        type: "Revolve",
        sketch: sketchId,
        axis_origin: axisOrigin,
        axis_dir: axisDir,
        angle_deg: angle_deg ?? 360,
      },
    };

    return revolveId;
  }

  if (isSweepPart(part)) {
    const { sketch, path, twist_deg, scale_start, scale_end } = part.sweep;
    const sketchId = createSketchNode(sketch, nodes, nextId);

    const sweepId = nextId.value++;

    if (path.type === "line") {
      nodes[String(sweepId)] = {
        id: sweepId,
        name: null,
        op: {
          type: "Sweep",
          sketch: sketchId,
          path: {
            type: "Line",
            start: path.start,
            end: path.end,
          },
          twist_angle: twist_deg ? (twist_deg * Math.PI) / 180 : undefined,
          scale_start,
          scale_end,
        },
      };
    } else {
      // Helix path
      const turns = path.height / path.pitch;
      nodes[String(sweepId)] = {
        id: sweepId,
        name: null,
        op: {
          type: "Sweep",
          sketch: sketchId,
          path: {
            type: "Helix",
            radius: path.radius,
            pitch: path.pitch,
            height: path.height,
            turns,
          },
          twist_angle: twist_deg ? (twist_deg * Math.PI) / 180 : undefined,
          scale_start,
          scale_end,
        },
      };
    }

    return sweepId;
  }

  if (isLoftPart(part)) {
    const { sketches, closed } = part.loft;
    const sketchIds = sketches.map((sketch) => createSketchNode(sketch, nodes, nextId));

    const loftId = nextId.value++;
    nodes[String(loftId)] = {
      id: loftId,
      name: null,
      op: {
        type: "Loft",
        sketches: sketchIds,
        closed,
      },
    };

    return loftId;
  }

  throw new Error("Part must have primitive, extrude, revolve, sweep, or loft definition");
}

/** Build an IR document from tool input. */
export function createCadDocument(
  input: unknown,
): { content: Array<{ type: "text"; text: string }> } {
  const { parts } = input as CreateInput;
  const doc = createDocument();
  const nextId = { value: 1 };
  const partRootMap = new Map<string, NodeId>();

  for (const part of parts) {
    // Create base geometry (primitive, extrude, revolve, sweep, or loft)
    let currentId = createPartBaseNode(part, doc.nodes, nextId);

    // Apply operations
    if (part.operations) {
      for (const op of part.operations) {
        const newId = nextId.value++;
        let newOp: CsgOp;

        switch (op.type) {
          case "union":
          case "difference":
          case "intersection": {
            if (!op.primitive) {
              throw new Error(`${op.type} requires a primitive`);
            }
            // Create the tool primitive
            let toolId = createPrimitiveNode(op.primitive, doc.nodes, nextId);
            // Translate it if needed
            if (op.at) {
              // Position resolution requires primitive-based parts
              if (!isPrimitivePart(part)) {
                // For non-primitive parts, treat 'at' as absolute position
                const pos = typeof op.at === "string"
                  ? { x: 0, y: 0, z: 0 } // Named positions need a primitive
                  : op.at as Vec3;
                const translateId = nextId.value++;
                doc.nodes[String(translateId)] = {
                  id: translateId,
                  name: null,
                  op: { type: "Translate", child: toolId, offset: pos },
                };
                toolId = translateId;
              } else {
                const resolvedPos = resolvePosition(op.at, part.primitive);
                const translateId = nextId.value++;
                doc.nodes[String(translateId)] = {
                  id: translateId,
                  name: null,
                  op: { type: "Translate", child: toolId, offset: resolvedPos },
                };
                toolId = translateId;
              }
            }
            newOp = {
              type: op.type === "union" ? "Union" : op.type === "difference" ? "Difference" : "Intersection",
              left: currentId,
              right: toolId,
            } as CsgOp;
            break;
          }

          case "hole": {
            if (!op.diameter) {
              throw new Error("hole requires a diameter");
            }
            if (!isPrimitivePart(part)) {
              throw new Error("hole operation requires a primitive-based part");
            }
            const radius = op.diameter / 2;
            const bbox = getPrimitiveBBox(part.primitive);
            const zExtent = bbox.max.z - bbox.min.z;
            // For through-hole, extend past part bounds; otherwise use specified depth
            const holeHeight = op.depth ?? zExtent + 2;

            // Create cylinder primitive for the hole
            const cylinderId = nextId.value++;
            doc.nodes[String(cylinderId)] = {
              id: cylinderId,
              name: null,
              op: {
                type: "Cylinder",
                radius,
                height: holeHeight,
                segments: 32,
              },
            };

            // Resolve position (default to center if not specified)
            // Note: isPrimitivePart check above ensures part.primitive exists
            const pos = op.at ?? "center";
            const primitivePart = part as PrimitivePartInput;
            const resolvedPos = resolvePosition(pos, primitivePart.primitive);
            // For through-hole, position cylinder to start below the part
            const zOffset = op.depth ? resolvedPos.z : bbox.min.z - 1;

            const translateId = nextId.value++;
            doc.nodes[String(translateId)] = {
              id: translateId,
              name: null,
              op: {
                type: "Translate",
                child: cylinderId,
                offset: { x: resolvedPos.x, y: resolvedPos.y, z: zOffset },
              },
            };

            newOp = {
              type: "Difference",
              left: currentId,
              right: translateId,
            } as CsgOp;
            break;
          }

          case "translate":
            newOp = {
              type: "Translate",
              child: currentId,
              offset: op.offset ?? { x: 0, y: 0, z: 0 },
            };
            break;

          case "rotate":
            newOp = {
              type: "Rotate",
              child: currentId,
              angles: op.angles ?? { x: 0, y: 0, z: 0 },
            };
            break;

          case "scale":
            newOp = {
              type: "Scale",
              child: currentId,
              factor: op.factor ?? { x: 1, y: 1, z: 1 },
            };
            break;

          case "linear_pattern":
            newOp = {
              type: "LinearPattern",
              child: currentId,
              direction: op.direction ?? { x: 1, y: 0, z: 0 },
              count: op.count ?? 2,
              spacing: op.spacing ?? 10,
            };
            break;

          case "circular_pattern":
            newOp = {
              type: "CircularPattern",
              child: currentId,
              axis_origin: op.axis_origin ?? { x: 0, y: 0, z: 0 },
              axis_dir: op.axis_dir ?? { x: 0, y: 0, z: 1 },
              count: op.count ?? 4,
              angle_deg: op.angle_deg ?? 360,
            };
            break;

          case "fillet":
            newOp = {
              type: "Fillet",
              child: currentId,
              radius: op.radius ?? 1,
            };
            break;

          case "chamfer":
            newOp = {
              type: "Chamfer",
              child: currentId,
              distance: op.distance ?? 1,
            };
            break;

          case "shell":
            newOp = {
              type: "Shell",
              child: currentId,
              thickness: op.thickness ?? 2,
            };
            break;

          default:
            throw new Error(`Unsupported operation type: ${op.type}`);
        }

        doc.nodes[String(newId)] = { id: newId, name: part.name, op: newOp };
        currentId = newId;
      }
    }

    // Set the name on the final node
    const finalNode = doc.nodes[String(currentId)];
    if (finalNode) {
      finalNode.name = part.name;
    }

    // Add to roots
    doc.roots.push({
      root: currentId,
      material: part.material ?? "default",
    });

    // Track material assignment
    doc.part_materials[part.name] = part.material ?? "default";

    // Track root ID by part name for assembly lookup
    partRootMap.set(part.name, currentId);
  }

  // Process assembly if provided
  const { assembly } = input as CreateInput;
  if (assembly?.instances?.length) {
    // Create partDefs from parts
    doc.partDefs = {};
    for (const [partName, rootId] of partRootMap) {
      const partDef = parts.find((p) => p.name === partName);
      doc.partDefs[partName] = {
        id: partName,
        name: partName,
        root: rootId,
        defaultMaterial: partDef?.material ?? "default",
      };
    }

    // Create instances
    doc.instances = assembly.instances.map((inst) => {
      const partDef = doc.partDefs![inst.part];
      if (!partDef) {
        throw new Error(`Instance ${inst.id} references unknown part: ${inst.part}`);
      }

      return {
        id: inst.id,
        partDefId: inst.part,
        name: inst.name ?? inst.id,
        transform: inst.position || inst.rotation
          ? {
              translation: inst.position ?? { x: 0, y: 0, z: 0 },
              rotation: inst.rotation ?? { x: 0, y: 0, z: 0 },
              scale: { x: 1, y: 1, z: 1 },
            }
          : undefined,
        material: undefined,
      };
    });

    // Create joints
    doc.joints = assembly.joints.map((joint) => {
      // Convert axis to Vec3
      let axisVec: Vec3;
      if (typeof joint.axis === "string" || joint.axis === undefined) {
        const axisName = joint.axis ?? "z";
        switch (axisName) {
          case "x":
            axisVec = { x: 1, y: 0, z: 0 };
            break;
          case "y":
            axisVec = { x: 0, y: 1, z: 0 };
            break;
          case "z":
            axisVec = { x: 0, y: 0, z: 1 };
            break;
        }
      } else {
        axisVec = joint.axis;
      }

      // Convert joint type to JointKind
      let kind: import("@vcad/ir").JointKind;
      switch (joint.type) {
        case "fixed":
          kind = { type: "Fixed" };
          break;
        case "revolute":
          kind = {
            type: "Revolute",
            axis: axisVec,
            limits: joint.limits,
          };
          break;
        case "slider":
          kind = {
            type: "Slider",
            axis: axisVec,
            limits: joint.limits,
          };
          break;
        case "cylindrical":
          kind = { type: "Cylindrical", axis: axisVec };
          break;
        case "ball":
          kind = { type: "Ball" };
          break;
      }

      return {
        id: joint.id,
        name: joint.name,
        parentInstanceId: joint.parent,
        childInstanceId: joint.child,
        parentAnchor: joint.parent_anchor ?? { x: 0, y: 0, z: 0 },
        childAnchor: joint.child_anchor ?? { x: 0, y: 0, z: 0 },
        kind,
        state: joint.state ?? 0,
      };
    });

    // Set ground instance
    if (assembly.ground) {
      doc.groundInstanceId = assembly.ground;
    }

    // In assembly mode, clear roots (instances take over)
    doc.roots = [];
  }

  // Format output (default to compact for token efficiency)
  const { format = "vcode" } = input as CreateInput;
  const text = format === "json" ? JSON.stringify(doc, null, 2) : toVCode(doc);

  return {
    content: [
      {
        type: "text",
        text,
      },
    ],
  };
}
