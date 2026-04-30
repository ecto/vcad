/**
 * v2 type vocabulary — the projection of IR that the agent-facing tools
 * speak. These are the JSON shapes wire schemas validate against; the
 * desugarer (in `build/desugar.ts`) lowers them to IR.
 *
 * Names mirror the design doc (`docs/design/mcp-v2.md`) verbatim. Keep
 * them stable: new optional fields are fine, renames are not.
 */

import type {
  Document,
  Vec2,
  Vec3,
  MaterialDef,
  PathCurve as IrPathCurve,
} from "@vcad/ir";
import type { DocHandle } from "./handles.js";

export type DocRef = DocHandle | Document;

/** Embedded blob (base64) or fetchable URL. */
export type ResourceRef =
  | { kind: "embedded"; mime: string; data_base64: string }
  | { kind: "url"; url: string };

/** RGB triple in linear space, 0..1 components. */
export type RGB = [number, number, number];

/** Material definition — named library lookup or inline PBR. */
export type Material =
  | { kind: "named"; name: string }
  | {
      kind: "pbr";
      albedo: RGB;
      roughness: number;
      metallic: number;
      ior?: number;
      transmission?: number;
      emission?: RGB;
      clearcoat?: number;
      clearcoat_roughness?: number;
      sheen?: number;
      anisotropy?: number;
      subsurface?: number;
      normal_map?: ResourceRef;
      roughness_map?: ResourceRef;
      albedo_map?: ResourceRef;
    };

// ── Reference vocabulary ────────────────────────────────────────────
// Human-stable references over opaque kernel ids. The desugarer resolves
// every ref against the active doc just before applying an op.

export type NodeRef = string;

export type FaceRef =
  | { node: NodeRef; face: number }
  | { node: NodeRef; face_named: string }
  | { node: NodeRef; face_at: Vec3 }
  | { node: NodeRef; face_normal: Vec3 }
  | {
      node: NodeRef;
      face_role: "top" | "bottom" | "front" | "back" | "left" | "right";
    };

export type EdgeRef =
  | { node: NodeRef; edge: number }
  | {
      node: NodeRef;
      edges_role: "top_outer" | "all_top" | "fillet_loops" | "all";
    }
  | { between_faces: [FaceRef, FaceRef] };

export type AxisRef =
  | { kind: "x" | "y" | "z" }
  | { axis_named: string }
  | { from: Vec3; to: Vec3 }
  | { node: NodeRef; axis_of: "cylinder" | "cone" };

export type PlaneRef =
  | { kind: "xy" | "xz" | "yz" }
  | { plane_named: string }
  | { node: NodeRef; face_named: string }
  | { offset: { from: PlaneRef; distance: number } };

/** Position shorthand: literal, percent-of-bbox, or named anchor. */
export type NamedPos =
  | "center"
  | "top-center"
  | "bottom-center"
  | { x: `${number}%` | number; y: `${number}%` | number; z?: `${number}%` | number };

// ── Sketches ────────────────────────────────────────────────────────

export type EntitySelector =
  | { id: string }
  | { id: string; point: "start" | "end" | "center" };

export type SketchEntity =
  | { id: string; kind: "line"; from: Vec2; to: Vec2 }
  | {
      id: string;
      kind: "arc";
      center: Vec2;
      radius: number;
      start_angle: number;
      end_angle: number;
    }
  | { id: string; kind: "circle"; center: Vec2; radius: number }
  | {
      id: string;
      kind: "ellipse";
      center: Vec2;
      rx: number;
      ry: number;
      rotation_deg?: number;
    }
  | { id: string; kind: "spline"; control_points: Vec2[]; degree?: 2 | 3 | 5 }
  | {
      id: string;
      kind: "polygon";
      center: Vec2;
      sides: number;
      radius: number;
      rotation_deg?: number;
    }
  | { id: string; kind: "rectangle"; corner: Vec2; size: Vec2 }
  | {
      id: string;
      kind: "text";
      at: Vec2;
      text: string;
      height: number;
      font?: string;
    };

export type SketchConstraint =
  | { kind: "coincident"; a: EntitySelector; b: EntitySelector }
  | { kind: "horizontal"; entity: EntitySelector }
  | { kind: "vertical"; entity: EntitySelector }
  | { kind: "parallel"; a: EntitySelector; b: EntitySelector }
  | { kind: "perpendicular"; a: EntitySelector; b: EntitySelector }
  | { kind: "tangent"; a: EntitySelector; b: EntitySelector }
  | { kind: "fixed"; entity: EntitySelector }
  | { kind: "distance"; a: EntitySelector; b: EntitySelector; value: number }
  | { kind: "length"; entity: EntitySelector; value: number }
  | { kind: "equal_length"; a: EntitySelector; b: EntitySelector }
  | { kind: "radius"; entity: EntitySelector; value: number }
  | { kind: "angle"; a: EntitySelector; b: EntitySelector; value_deg: number };

export type SketchDimension = SketchConstraint;

// ── Holes & threads ─────────────────────────────────────────────────

export type HoleKind = "through" | "blind" | "tapped";

export type ThreadSpec =
  | {
      standard: "metric";
      designation: string;
      depth: number;
      class?: "6g" | "6H";
    }
  | {
      standard: "unc" | "unf";
      designation: string;
      depth: number;
      class?: "2A" | "2B" | "3A" | "3B";
    }
  | {
      custom: {
        pitch: number;
        major_diameter: number;
        minor_diameter: number;
        depth: number;
      };
    };

// ── Plane / axis defs (for `ref_plane` / `ref_axis`) ────────────────

export type PlaneDef =
  | { kind: "xy" | "xz" | "yz"; offset?: number }
  | { origin: Vec3; normal: Vec3 }
  | { three_points: [Vec3, Vec3, Vec3] };

export type AxisDef =
  | { kind: "x" | "y" | "z"; through?: Vec3 }
  | { from: Vec3; to: Vec3 };

// ── BuildOp — discriminated union covering the full kernel ──────────

export type BuildOp =
  // Primitives
  | { op: "primitive"; kind: "cube"; size: Vec3; at?: Vec3 | NamedPos; name?: string; material?: string }
  | {
      op: "primitive";
      kind: "cylinder";
      radius: number;
      height: number;
      at?: Vec3 | NamedPos;
      segments?: number;
      name?: string;
      material?: string;
    }
  | {
      op: "primitive";
      kind: "sphere";
      radius: number;
      at?: Vec3 | NamedPos;
      segments?: number;
      name?: string;
      material?: string;
    }
  | {
      op: "primitive";
      kind: "cone";
      radius_bottom: number;
      radius_top: number;
      height: number;
      at?: Vec3 | NamedPos;
      segments?: number;
      name?: string;
      material?: string;
    }
  | {
      op: "primitive";
      kind: "torus";
      major_radius: number;
      minor_radius: number;
      at?: Vec3 | NamedPos;
      name?: string;
      material?: string;
    }
  | { op: "primitive"; kind: "wedge"; size: Vec3; at?: Vec3 | NamedPos; name?: string; material?: string }

  // Sketches
  | {
      op: "sketch";
      name: string;
      plane: PlaneRef;
      entities: SketchEntity[];
      constraints?: SketchConstraint[];
      dimensions?: SketchDimension[];
    }

  // Sketch-based features
  | {
      op: "extrude";
      sketch: string;
      depth: number | "through_all" | { to_face: FaceRef };
      direction?: "normal" | "reverse" | "both";
      draft_deg?: number;
      thin?: { thickness: number };
      name?: string;
      material?: string;
    }
  | {
      op: "revolve";
      sketch: string;
      axis: AxisRef;
      angle_deg: number;
      direction?: "normal" | "reverse";
      name?: string;
      material?: string;
    }
  | {
      op: "sweep";
      profile: string;
      path: string | IrPathCurve;
      twist_deg?: number;
      align?: "frenet" | "fixed";
      name?: string;
      material?: string;
    }
  | {
      op: "loft";
      profiles: string[];
      guides?: string[];
      closed?: boolean;
      ruled?: boolean;
      name?: string;
      material?: string;
    }
  | {
      op: "hole";
      at: Vec3 | FaceRef;
      kind: HoleKind;
      diameter: number;
      depth: number | "through";
      counterbore?: { diameter: number; depth: number };
      countersink?: { diameter: number; angle_deg: number };
      thread?: ThreadSpec;
      target?: NodeRef;
      name?: string;
    }

  // Booleans
  | { op: "union"; subjects: NodeRef[]; name?: string; material?: string }
  | { op: "difference"; subject: NodeRef; tools: NodeRef[]; name?: string; material?: string }
  | { op: "intersection"; subjects: NodeRef[]; name?: string; material?: string }

  // Modify features
  | {
      op: "fillet";
      target?: NodeRef;
      edges: EdgeRef[];
      radius: number;
      variable?: { start: number; end: number };
      name?: string;
    }
  | {
      op: "chamfer";
      target?: NodeRef;
      edges: EdgeRef[];
      distance: number;
      angle_deg?: number;
      name?: string;
    }
  | {
      op: "shell";
      target?: NodeRef;
      thickness: number;
      remove_faces?: FaceRef[];
      outward?: boolean;
      name?: string;
    }
  | {
      op: "draft";
      target?: NodeRef;
      faces: FaceRef[];
      angle_deg: number;
      pull_direction: AxisRef;
      name?: string;
    }

  // Patterns
  | {
      op: "linear_pattern";
      subjects: NodeRef[];
      direction: Vec3;
      count: number;
      spacing: number;
      symmetric?: boolean;
      second_direction?: { direction: Vec3; count: number; spacing: number };
      name?: string;
    }
  | {
      op: "circular_pattern";
      subjects: NodeRef[];
      axis: AxisRef;
      count: number;
      angle_deg?: number;
      equal_spacing?: boolean;
      name?: string;
    }
  | { op: "mirror"; subjects: NodeRef[]; plane: PlaneRef; merge?: boolean; name?: string }

  // Transforms
  | { op: "translate"; subjects: NodeRef[]; offset: Vec3; name?: string }
  | { op: "rotate"; subjects: NodeRef[]; axis: AxisRef; angle_deg: number; name?: string }
  | { op: "scale"; subjects: NodeRef[]; factor: number | Vec3; about?: Vec3; name?: string }

  // Reference geometry
  | { op: "ref_plane"; name: string; def: PlaneDef }
  | { op: "ref_axis"; name: string; def: AxisDef }
  | { op: "ref_point"; name: string; at: Vec3 }

  // Sheet metal
  | { op: "sheet_base"; sketch: string; thickness: number; bend_radius: number; name?: string; material?: string }
  | {
      op: "sheet_flange";
      target?: NodeRef;
      edge: EdgeRef;
      angle_deg: number;
      length: number;
      relief?: "rectangular" | "obround";
      name?: string;
    }
  | { op: "sheet_unfold"; subject: NodeRef; name?: string }

  // Materials & metadata
  | { op: "set_material"; subjects: NodeRef[]; material: string | Material }
  | { op: "rename"; node: NodeRef; name: string }
  | { op: "delete"; subjects: NodeRef[] }
  | { op: "set_parameter"; name: string; value: number | string }

  // Escape hatch — for IR-fluent agents.
  | { op: "raw_ir"; nodes: import("@vcad/ir").Node[]; roots?: number[] };

// ── Build tool envelope shapes ──────────────────────────────────────

export interface BuildInput {
  doc?: DocRef;
  ops: BuildOp[];
  materials?: Record<string, Material | MaterialDef>;
  parameters?: Record<string, number | string>;
  metadata?: { name?: string; description?: string; units?: "mm" | "in" };
}

export interface BuildResult {
  added_nodes: number[];
  modified_nodes: number[];
  removed_nodes: number[];
  /** Optional sketch solver report (when the op set contained sketch ops). */
  solver?: {
    status: "fully_constrained" | "under_constrained" | "over_constrained" | "failed";
    dof: number;
  };
  /** Map of declared NodeRef name → resolved node id. */
  named_nodes?: Record<string, number>;
}
