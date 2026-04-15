import type { NodeId, Vec2, Vec3, SketchSegment2D, SketchConstraint } from "@vcad/ir";
import { vec3Cross, vec3Normalize } from "@vcad/ir";

export type PrimitiveKind = "cube" | "cylinder" | "sphere";
export type BooleanType = "union" | "difference" | "intersection";

/** Axis-aligned sketch plane */
export type AxisAlignedPlane = "XY" | "XZ" | "YZ";

/** Arbitrary sketch plane defined by face selection */
export interface ArbitraryPlane {
  type: "face";
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
  normal: Vec3;
}

/** Sketch plane - can be axis-aligned or arbitrary (from face) */
export type SketchPlane = AxisAlignedPlane | ArbitraryPlane;

/** Information about a selected face */
export interface FaceInfo {
  partId: string;
  faceIndex: number;
  normal: Vec3;
  centroid: Vec3;
  /** Unique vertices on this coplanar face, in kernel (Z-up) coordinates.
   *  Used to size the sketch grid and provide snap targets. Optional for
   *  back-compat with hover paths that only need normal+centroid. */
  vertices?: Vec3[];
}

export interface PrimitivePartInfo {
  id: string;
  name: string;
  kind: PrimitiveKind;
  primitiveNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface BooleanPartInfo {
  id: string;
  name: string;
  kind: "boolean";
  booleanType: BooleanType;
  booleanNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
  sourcePartIds: [string, string];
}

export interface ExtrudePartInfo {
  id: string;
  name: string;
  kind: "extrude";
  sketchNodeId: NodeId;
  extrudeNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface RevolvePartInfo {
  id: string;
  name: string;
  kind: "revolve";
  sketchNodeId: NodeId;
  revolveNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface SweepPartInfo {
  id: string;
  name: string;
  kind: "sweep";
  sketchNodeId: NodeId;
  sweepNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface LoftPartInfo {
  id: string;
  name: string;
  kind: "loft";
  sketchNodeIds: NodeId[];
  loftNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface ImportedMeshPartInfo {
  id: string;
  name: string;
  kind: "imported-mesh";
  meshNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
  /** Source filename for display */
  source?: string;
}

export interface FilletPartInfo {
  id: string;
  name: string;
  kind: "fillet";
  sourcePartId: string;
  filletNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface ChamferPartInfo {
  id: string;
  name: string;
  kind: "chamfer";
  sourcePartId: string;
  chamferNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface ShellPartInfo {
  id: string;
  name: string;
  kind: "shell";
  sourcePartId: string;
  shellNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface LinearPatternPartInfo {
  id: string;
  name: string;
  kind: "linear-pattern";
  sourcePartId: string;
  patternNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface CircularPatternPartInfo {
  id: string;
  name: string;
  kind: "circular-pattern";
  sourcePartId: string;
  patternNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface MirrorPartInfo {
  id: string;
  name: string;
  kind: "mirror";
  sourcePartId: string;
  mirrorNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface TextPartInfo {
  id: string;
  name: string;
  kind: "text";
  textNodeId: NodeId;
  extrudeNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface PcbBoardPartInfo {
  id: string;
  name: string;
  kind: "pcb-board";
  boardNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export interface EmbroideryPatternPartInfo {
  id: string;
  name: string;
  kind: "embroidery-pattern";
  patternNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
  source?: string;
}

export interface StitchPartInfo {
  id: string;
  name: string;
  kind: "stitch";
  sourcePartId: string;
  patternNodeId: NodeId;
  scaleNodeId: NodeId;
  rotateNodeId: NodeId;
  translateNodeId: NodeId;
}

export type PartInfo = PrimitivePartInfo | BooleanPartInfo | ExtrudePartInfo | RevolvePartInfo | SweepPartInfo | LoftPartInfo | ImportedMeshPartInfo | FilletPartInfo | ChamferPartInfo | ShellPartInfo | LinearPatternPartInfo | CircularPatternPartInfo | MirrorPartInfo | TextPartInfo | PcbBoardPartInfo | EmbroideryPatternPartInfo | StitchPartInfo;

export function isPrimitivePart(part: PartInfo): part is PrimitivePartInfo {
  return part.kind === "cube" || part.kind === "cylinder" || part.kind === "sphere";
}

export function isBooleanPart(part: PartInfo): part is BooleanPartInfo {
  return part.kind === "boolean";
}

export function isExtrudePart(part: PartInfo): part is ExtrudePartInfo {
  return part.kind === "extrude";
}

export function isRevolvePart(part: PartInfo): part is RevolvePartInfo {
  return part.kind === "revolve";
}

export function isSweepPart(part: PartInfo): part is SweepPartInfo {
  return part.kind === "sweep";
}

export function isLoftPart(part: PartInfo): part is LoftPartInfo {
  return part.kind === "loft";
}

export function isImportedMeshPart(part: PartInfo): part is ImportedMeshPartInfo {
  return part.kind === "imported-mesh";
}

export function isFilletPart(part: PartInfo): part is FilletPartInfo {
  return part.kind === "fillet";
}

export function isChamferPart(part: PartInfo): part is ChamferPartInfo {
  return part.kind === "chamfer";
}

export function isShellPart(part: PartInfo): part is ShellPartInfo {
  return part.kind === "shell";
}

export function isLinearPatternPart(part: PartInfo): part is LinearPatternPartInfo {
  return part.kind === "linear-pattern";
}

export function isCircularPatternPart(part: PartInfo): part is CircularPatternPartInfo {
  return part.kind === "circular-pattern";
}

export function isMirrorPart(part: PartInfo): part is MirrorPartInfo {
  return part.kind === "mirror";
}

export function isTextPart(part: PartInfo): part is TextPartInfo {
  return part.kind === "text";
}

export function isPcbBoardPart(part: PartInfo): part is PcbBoardPartInfo {
  return part.kind === "pcb-board";
}

export function isEmbroideryPatternPart(part: PartInfo): part is EmbroideryPatternPartInfo {
  return part.kind === "embroidery-pattern";
}

export function isStitchPart(part: PartInfo): part is StitchPartInfo {
  return part.kind === "stitch";
}

/** Returns true if a part can be converted to embroidery stitches (text, extrude, revolve, sweep, loft). */
export function isStitchEligible(part: PartInfo): boolean {
  return part.kind === "text" || part.kind === "extrude" || part.kind === "revolve" || part.kind === "sweep" || part.kind === "loft";
}

export type ToolMode = "select" | "primitive" | "simulate";
export type TransformMode = "translate" | "rotate" | "scale";
export type Theme = "system" | "dark" | "light";

/** Constraint tool types */
export type ConstraintTool =
  | "none"
  | "horizontal"
  | "vertical"
  | "distance"
  | "coincident"
  | "parallel"
  | "perpendicular"
  | "length"
  | "fixed"
  | "equal";

/** Constraint status for visual feedback */
export type ConstraintStatus = "under" | "solved" | "over" | "error";

/** Sketch editing state */
export interface SketchState {
  /** Whether sketch mode is active */
  active: boolean;
  /** The plane the sketch is on */
  plane: SketchPlane;
  /** Origin point of the sketch plane */
  origin: Vec3;
  /** Segments drawn so far */
  segments: SketchSegment2D[];
  /** Constraints on the sketch */
  constraints: SketchConstraint[];
  /** Current drawing tool */
  tool: "line" | "rectangle" | "circle";
  /** Current constraint tool (when applying constraints) */
  constraintTool: ConstraintTool;
  /** Points accumulated for current shape */
  points: Vec2[];
  /** Selected segment indices (for applying constraints) */
  selectedSegments: number[];
  /** Whether sketch is solved (constraints satisfied) */
  solved: boolean;
  /** Visual feedback status for constraints */
  constraintStatus: ConstraintStatus;
}

/** Get the X and Y direction vectors for a sketch plane */
export function getSketchPlaneDirections(plane: SketchPlane): { x_dir: Vec3; y_dir: Vec3; normal: Vec3 } {
  if (typeof plane === "string") {
    switch (plane) {
      case "XY":
        return { x_dir: { x: 1, y: 0, z: 0 }, y_dir: { x: 0, y: 1, z: 0 }, normal: { x: 0, y: 0, z: 1 } };
      case "XZ":
        return { x_dir: { x: 1, y: 0, z: 0 }, y_dir: { x: 0, y: 0, z: -1 }, normal: { x: 0, y: 1, z: 0 } };
      case "YZ":
        return { x_dir: { x: 0, y: 1, z: 0 }, y_dir: { x: 0, y: 0, z: 1 }, normal: { x: 1, y: 0, z: 0 } };
    }
  }
  // Arbitrary plane from face selection
  return { x_dir: plane.xDir, y_dir: plane.yDir, normal: plane.normal };
}

/** Check if a plane is axis-aligned */
export function isAxisAlignedPlane(plane: SketchPlane): plane is AxisAlignedPlane {
  return typeof plane === "string";
}

/** Compute a sketch plane from a face selection */
export function computePlaneFromFace(face: FaceInfo): ArbitraryPlane {
  const normal = vec3Normalize(face.normal);

  // Build orthonormal basis - pick reference vector that isn't parallel to normal
  const ref = Math.abs(normal.z) < 0.9 ? { x: 0, y: 0, z: 1 } : { x: 1, y: 0, z: 0 };
  const xDir = vec3Normalize(vec3Cross(ref, normal));
  const yDir = vec3Cross(normal, xDir);

  return { type: "face", origin: face.centroid, xDir, yDir, normal };
}

/** Get display name for a sketch plane */
export function getSketchPlaneName(plane: SketchPlane): string {
  if (typeof plane === "string") return plane;
  return "Face";
}

/** Format a normal vector as a human-readable direction string */
export function formatDirection(normal: Vec3): string {
  if (Math.abs(normal.x) > 0.9) return normal.x > 0 ? "+X" : "-X";
  if (Math.abs(normal.y) > 0.9) return normal.y > 0 ? "+Y" : "-Y";
  if (Math.abs(normal.z) > 0.9) return normal.z > 0 ? "+Z" : "-Z";
  return "custom";
}

/** Negate a direction string (e.g., "+X" → "-X") */
export function negateDirection(dir: string): string {
  if (dir.startsWith("+")) return "-" + dir.slice(1);
  if (dir.startsWith("-")) return "+" + dir.slice(1);
  return dir;
}
