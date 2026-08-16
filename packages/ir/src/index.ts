/**
 * @vcad/ir — Intermediate representation for the vcad CAD ecosystem.
 *
 * Mirrors the Rust `vcad-ir` crate types exactly for cross-language compatibility.
 */

export {
  vec3Add,
  vec3Sub,
  vec3Scale,
  vec3Dot,
  vec3Cross,
  vec3Length,
  vec3Normalize,
  vec3Negate,
  vec3Zero,
} from "./vec3.js";

/** Unique identifier for a node in the IR graph. */
export type NodeId = number;

// ===========================================================================
// Generated IR types — single source of truth is the Rust `vcad-ir` crate.
// Regenerate with: npm run ir:gen   (see packages/ir/src/generated.ts).
// CI runs `npm run ir:check` to fail builds where this has drifted from Rust.
// ===========================================================================
export * from "./generated.js";

// Local imports of the generated types so the hand-written behavior + aliases
// below can reference them by bare name (export * does not bind locally).
import type {
  AmbientOcclusion,
  Background,
  Bindings,
  Bloom,
  BoardOutline,
  CameraPreset,
  CsgOp,
  DesignRules,
  Document,
  DrillSpec,
  EmbroideryDesign,
  EmbroideryThread,
  Environment,
  EnvironmentPreset,
  Expr,
  FillParams,
  FillType,
  Footprint,
  FootprintGraphic,
  FootprintTemplate,
  InertialProperties,
  Instance,
  IrStitchGroup,
  Joint,
  JointKind,
  Keepout,
  LabelScope,
  LayerStackup,
  Light,
  LightKind,
  MaterialDef,
  Net,
  NetClassRules,
  NetTie,
  Node,
  Pad,
  PadShape,
  PadType,
  Parameter,
  PartDef,
  PartInfo,
  PathCurve,
  Pcb,
  PcbLayer,
  PinType,
  PostProcessing,
  SceneEntry,
  SceneSettings,
  SchematicComponent,
  SchematicJunction,
  SchematicLabel,
  SchematicPin,
  SchematicSheet,
  SchematicWire,
  SheetMetalDirection,
  SheetMetalHemKind,
  Silhouette,
  SketchSegment2D,
  StackupLayer,
  SymbolDef,
  SymbolGraphic,
  TextAlignment,
  ThermalReliefStyle,
  ToneMapping,
  ToolSchemaEntry,
  Trace,
  TraceArc,
  Transform3D,
  VcadFile,
  VcadFormat,
  Vec2,
  Vec3,
  Via,
  Vignette,
  Zone,
  ZoneFillType,
} from "./generated.js";

// --- Named CsgOp variant aliases (ergonomic narrowing; ts-rs inlines the union) ---
export type CubeOp = Extract<CsgOp, { type: "Cube" }>;
export type CylinderOp = Extract<CsgOp, { type: "Cylinder" }>;
export type SphereOp = Extract<CsgOp, { type: "Sphere" }>;
export type ConeOp = Extract<CsgOp, { type: "Cone" }>;
export type EmptyOp = Extract<CsgOp, { type: "Empty" }>;
export type UnionOp = Extract<CsgOp, { type: "Union" }>;
export type DifferenceOp = Extract<CsgOp, { type: "Difference" }>;
export type IntersectionOp = Extract<CsgOp, { type: "Intersection" }>;
export type TranslateOp = Extract<CsgOp, { type: "Translate" }>;
export type RotateOp = Extract<CsgOp, { type: "Rotate" }>;
export type ScaleOp = Extract<CsgOp, { type: "Scale" }>;
export type Sketch2DOp = Extract<CsgOp, { type: "Sketch2D" }>;
export type ExtrudeOp = Extract<CsgOp, { type: "Extrude" }>;
export type RevolveOp = Extract<CsgOp, { type: "Revolve" }>;
export type LinearPatternOp = Extract<CsgOp, { type: "LinearPattern" }>;
export type CircularPatternOp = Extract<CsgOp, { type: "CircularPattern" }>;
export type ShellOp = Extract<CsgOp, { type: "Shell" }>;
export type FilletOp = Extract<CsgOp, { type: "Fillet" }>;
export type ChamferOp = Extract<CsgOp, { type: "Chamfer" }>;
export type Text2DOp = Extract<CsgOp, { type: "Text2D" }>;
export type ImportedMeshOp = Extract<CsgOp, { type: "ImportedMesh" }>;
export type StepImportOp = Extract<CsgOp, { type: "step_import" }>;
export type SweepOp = Extract<CsgOp, { type: "Sweep" }>;
export type LoftOp = Extract<CsgOp, { type: "Loft" }>;
export type PcbBoardOp = Extract<CsgOp, { type: "PcbBoard" }>;
export type EmbroideryPatternOp = Extract<CsgOp, { type: "EmbroideryPattern" }>;
export type PartInstanceOp = Extract<CsgOp, { type: "PartInstance" }>;
export type SheetMetalBaseFlangeRectOp = Extract<CsgOp, { type: "SheetMetalBaseFlangeRect" }>;
export type SheetMetalEdgeFlangeOp = Extract<CsgOp, { type: "SheetMetalEdgeFlange" }>;
export type SheetMetalBaseFlangePolygonOp = Extract<CsgOp, { type: "SheetMetalBaseFlangePolygon" }>;
export type SheetMetalJogOp = Extract<CsgOp, { type: "SheetMetalJog" }>;
export type SheetMetalBendReliefOp = Extract<CsgOp, { type: "SheetMetalBendRelief" }>;
export type SheetMetalHemOp = Extract<CsgOp, { type: "SheetMetalHem" }>;

// --- Sketch / path / light / environment / background variant aliases ---
export type LineSegment2D = Extract<SketchSegment2D, { type: "Line" }>;
export type ArcSegment2D = Extract<SketchSegment2D, { type: "Arc" }>;
export type LinePath = Extract<PathCurve, { type: "Line" }>;
export type HelixPath = Extract<PathCurve, { type: "Helix" }>;
export type DirectionalLight = Extract<LightKind, { type: "Directional" }>;
export type PointLight = Extract<LightKind, { type: "Point" }>;
export type SpotLight = Extract<LightKind, { type: "Spot" }>;
export type AreaLight = Extract<LightKind, { type: "Area" }>;
export type PresetEnvironment = Extract<Environment, { type: "Preset" }>;
export type CustomEnvironment = Extract<Environment, { type: "Custom" }>;
export type NoEnvironment = Extract<Environment, { type: "None" }>;
export type EnvironmentBackground = Extract<Background, { type: "Environment" }>;
export type SolidBackground = Extract<Background, { type: "Solid" }>;
export type GradientBackground = Extract<Background, { type: "Gradient" }>;
export type TransparentBackground = Extract<Background, { type: "Transparent" }>;

// --- Name aliases (the Rust/serde names differ from the prior TS names) ---
export type StitchFillType = FillType;
export type JointLimits = [number, number];
export type PartInstance = Instance;

/** Create an identity transform (no translation, rotation, or scaling). */
export function identityTransform(): Transform3D {
  return {
    translation: { x: 0, y: 0, z: 0 },
    rotation: { x: 0, y: 0, z: 0 },
    scale: { x: 1, y: 1, z: 1 },
  };
}

// --- Sketch Constraints ---

/** Reference to a point within a sketch entity. */
export type EntityRef =
  | { type: "Point"; index: number }
  | { type: "LineStart"; index: number }
  | { type: "LineEnd"; index: number }
  | { type: "ArcStart"; index: number }
  | { type: "ArcEnd"; index: number }
  | { type: "Center"; index: number };

/** Coincident constraint - two points at the same location. */
export interface CoincidentConstraint {
  type: "Coincident";
  pointA: EntityRef;
  pointB: EntityRef;
}

/** Horizontal constraint - line parallel to X axis. */
export interface HorizontalConstraint {
  type: "Horizontal";
  line: number;
}

/** Vertical constraint - line parallel to Y axis. */
export interface VerticalConstraint {
  type: "Vertical";
  line: number;
}

/** Parallel constraint - two lines are parallel. */
export interface ParallelConstraint {
  type: "Parallel";
  lineA: number;
  lineB: number;
}

/** Perpendicular constraint - two lines are perpendicular. */
export interface PerpendicularConstraint {
  type: "Perpendicular";
  lineA: number;
  lineB: number;
}

/** Fixed constraint - point at a fixed position. */
export interface FixedConstraint {
  type: "Fixed";
  point: EntityRef;
  x: number;
  y: number;
}

/** Distance constraint - distance between two points. */
export interface DistanceConstraint {
  type: "Distance";
  pointA: EntityRef;
  pointB: EntityRef;
  distance: number;
}

/** Length constraint - length of a line. */
export interface LengthConstraint {
  type: "Length";
  line: number;
  length: number;
}

/** Equal length constraint - two lines have same length. */
export interface EqualLengthConstraint {
  type: "EqualLength";
  lineA: number;
  lineB: number;
}

/** Radius constraint - circle/arc has specific radius. */
export interface RadiusConstraint {
  type: "Radius";
  circle: number;
  radius: number;
}

/** Angle constraint - angle between two lines. */
export interface AngleConstraint {
  type: "Angle";
  lineA: number;
  lineB: number;
  angleDeg: number;
}

/** A constraint on sketch entities. */
export type SketchConstraint =
  | CoincidentConstraint
  | HorizontalConstraint
  | VerticalConstraint
  | ParallelConstraint
  | PerpendicularConstraint
  | FixedConstraint
  | DistanceConstraint
  | LengthConstraint
  | EqualLengthConstraint
  | RadiusConstraint
  | AngleConstraint;

// --- New CsgOp variant aliases (Torus/Wedge/Prism/Mirror) ---
export type TorusOp = Extract<CsgOp, { type: "Torus" }>;
export type WedgeOp = Extract<CsgOp, { type: "Wedge" }>;
export type PrismOp = Extract<CsgOp, { type: "Prism" }>;
export type MirrorOp = Extract<CsgOp, { type: "Mirror" }>;

/** Default fill parameters (manual / no auto-fill). */
export const DEFAULT_FILL_PARAMS: FillParams = {
  fill_type: "manual",
  angle_deg: 0,
  density_mm: 0.4,
  underlay: false,
  max_stitch_length_mm: 7.0,
};

// ============================================================================
// ECAD (PCB/Schematic) types
// ============================================================================

// ============================================================================
// Length Tuning / Meander types (mirrors vcad-ecad-pcb router)
// ============================================================================

/** Meander pattern style. */
export type MeanderStyle = "Trombone" | "Sawtooth";

/** Length tuning parameters for meander generation. */
export interface LengthTuneParams {
  /** Target trace length in mm. */
  target_length: number;
  /** Maximum meander amplitude in mm. */
  max_amplitude: number;
  /** Meander spacing in mm. */
  spacing: number;
  /** Meander pattern style. */
  style: MeanderStyle;
}

/** A meander segment to insert into a trace. */
export interface MeanderSegment {
  /** Meander waypoints in board coordinates. */
  points: Vec2[];
  /** Total added length from this meander. */
  added_length: number;
  /** Which segment of the input polyline this replaces. */
  segment_index: number;
}

/** Create a new empty document. */
export function createDocument(): Document {
  return {
    version: "0.1",
    nodes: {},
    materials: {},
    part_materials: {},
    roots: [],
  };
}

/** Serialize a document to a JSON string. */
export function toJson(doc: Document): string {
  return JSON.stringify(doc, null, 2);
}

/** Deserialize a document from a JSON string. */
export function fromJson(json: string): Document {
  return JSON.parse(json) as Document;
}

// ============================================================================
// VCode Format v0.2 (for cad0 model training and inference)
// ============================================================================

/** Current VCode format version. */
export const VCODE_VERSION = '0.2';

/**
 * Parse VCode text format into a vcad IR Document.
 *
 * The VCode format is a token-efficient text representation designed
 * for ML model training and inference. Supports the full v0.2 format with
 * materials, assembly, joints, scene settings, and node names.
 *
 * @example
 * ```typescript
 * const ir = "# vcad 0.2\nM default 0.8 0.8 0.8 0 0.5\nC 50 30 5\nROOT 0 default";
 * const doc = fromVCode(ir);
 * ```
 */
export function fromVCode(vcode: string): Document {
  const doc = createDocument();
  const lines = vcode.split('\n');
  let i = 0;
  let geometryNodeCount = 0;

  while (i < lines.length) {
    const line = lines[i].trim();

    // Skip empty lines and comments
    if (!line || line.startsWith('#')) {
      i++;
      continue;
    }

    const parts = splitLineRespectingQuotes(line);
    if (parts.length === 0) {
      i++;
      continue;
    }

    const opcode = parts[0];

    switch (opcode) {
      case 'M':
        parseMaterial(doc, parts, i);
        break;

      case 'ROOT':
        parseRoot(doc, parts, i);
        break;

      case 'PDEF':
        parsePartDef(doc, parts, i);
        break;

      case 'INST':
        parseInstance(doc, parts, i);
        break;

      case 'JFIX':
      case 'JREV':
      case 'JSLD':
      case 'JCYL':
      case 'JBAL':
      case 'JFRE':
        parseJoint(doc, opcode, parts, i);
        break;

      case 'GROUND':
        if (parts.length !== 2) {
          throw new VCodeParseError(i, `GROUND requires 1 arg, got ${parts.length - 1}`);
        }
        doc.groundInstanceId = parseStringArg(parts[1]);
        break;

      case 'ENV':
        parseEnvironment(doc, parts, i);
        break;

      case 'BG':
        parseBackground(doc, parts, i);
        break;

      case 'LDIR':
      case 'LPNT':
      case 'LSPT':
      case 'LAREA':
        parseLight(doc, opcode, parts, i);
        break;

      case 'AO':
        parseAO(doc, parts, i);
        break;

      case 'BLOOM':
        parseBloom(doc, parts, i);
        break;

      case 'VIG':
        parseVignette(doc, parts, i);
        break;

      case 'TONE':
        parseToneMapping(doc, parts, i);
        break;

      case 'EXP':
        parseExposure(doc, parts, i);
        break;

      case 'CAM':
        parseCamera(doc, parts, i);
        break;

      default: {
        // Geometry opcode
        const nodeId = geometryNodeCount;
        const [op, name, newIndex] = parseGeometryLine(line, i, lines);

        doc.nodes[nodeId.toString()] = {
          id: nodeId,
          name: name ?? null,
          op,
        };

        geometryNodeCount++;
        i = newIndex;
      }
    }

    i++;
  }

  // If no explicit ROOTs were defined, add a default one
  if (doc.roots.length === 0 && Object.keys(doc.nodes).length > 0) {
    const referenced = new Set<number>();
    for (const node of Object.values(doc.nodes)) {
      for (const childId of getChildren(node.op)) {
        referenced.add(childId);
      }
    }

    const rootId = Object.keys(doc.nodes)
      .map(Number)
      .filter(id => !referenced.has(id))
      .reduce((a, b) => Math.max(a, b), 0);

    // Add default material if none exists
    if (Object.keys(doc.materials).length === 0) {
      doc.materials['default'] = {
        name: 'default',
        color: [0.8, 0.8, 0.8],
        metallic: 0,
        roughness: 0.5,
      };
    }

    doc.roots.push({
      root: rootId,
      material: Object.keys(doc.materials)[0] ?? 'default',
    });
  }

  return doc;
}

/**
 * Convert a vcad IR Document to VCode text format (v0.2).
 *
 * @example
 * ```typescript
 * const compact = toVCode(doc);
 * console.log(compact); // "# vcad 0.2\n..."
 * ```
 */
export function toVCode(doc: Document): string {
  const lines: string[] = [];

  // Header
  lines.push(`# vcad ${VCODE_VERSION}`);
  lines.push('');

  // Materials section
  const matNames = Object.keys(doc.materials).sort();
  if (matNames.length > 0) {
    lines.push('# Materials');
    for (const name of matNames) {
      const mat = doc.materials[name];
      let line = `M ${escapeId(mat.name)} ${mat.color[0]} ${mat.color[1]} ${mat.color[2]} ${mat.metallic} ${mat.roughness}`;
      if (mat.density !== undefined) {
        line += ` ${mat.density}`;
        if (mat.friction !== undefined) {
          line += ` ${mat.friction}`;
        }
      }
      lines.push(line);
    }
    lines.push('');
  }

  // Geometry section
  const nodeIds = Object.keys(doc.nodes).map(Number);
  if (nodeIds.length > 0) {
    lines.push('# Geometry');

    // Find all root nodes (nodes not referenced by any other node)
    const referenced = new Set<number>();
    for (const node of Object.values(doc.nodes)) {
      for (const childId of getChildren(node.op)) {
        referenced.add(childId);
      }
    }

    const roots = nodeIds.filter(id => !referenced.has(id));

    // Topological sort
    const sorted = topologicalSort(doc, roots);

    // Create ID mapping
    const idMap = new Map<number, number>();
    sorted.forEach((id, index) => {
      idMap.set(id, index);
    });

    for (const nodeId of sorted) {
      const node = doc.nodes[nodeId.toString()];
      const line = formatOp(node.op, idMap, node.name ?? undefined);
      lines.push(line);
    }
    lines.push('');

    // Scene roots
    if (doc.roots.length > 0) {
      lines.push('# Scene');
      for (const entry of doc.roots) {
        const mappedId = idMap.get(entry.root);
        if (mappedId === undefined) {
          throw new Error(`Unknown root node ${entry.root}`);
        }
        let line = `ROOT ${mappedId} ${escapeId(entry.material)}`;
        if (entry.visible === false) {
          line += ' hidden';
        }
        lines.push(line);
      }
      lines.push('');
    }
  }

  // Part definitions
  if (doc.partDefs && Object.keys(doc.partDefs).length > 0) {
    lines.push('# Parts');

    // Rebuild idMap for part def node refs
    const referenced = new Set<number>();
    for (const node of Object.values(doc.nodes)) {
      for (const childId of getChildren(node.op)) {
        referenced.add(childId);
      }
    }
    const roots = Object.keys(doc.nodes).map(Number).filter(id => !referenced.has(id));
    const sorted = topologicalSort(doc, roots);
    const idMap = new Map<number, number>();
    sorted.forEach((id, index) => idMap.set(id, index));

    const pdefIds = Object.keys(doc.partDefs).sort();
    for (const id of pdefIds) {
      const pdef = doc.partDefs[id];
      const mappedRoot = idMap.get(pdef.root);
      if (mappedRoot === undefined) {
        throw new Error(`Unknown part def root ${pdef.root}`);
      }
      let line = `PDEF ${escapeId(pdef.id)} ${formatQuotedString(pdef.name ?? pdef.id)} ${mappedRoot}`;
      if (pdef.defaultMaterial) {
        line += ` ${escapeId(pdef.defaultMaterial)}`;
      }
      lines.push(line);
    }
    lines.push('');
  }

  // Instances
  if (doc.instances && doc.instances.length > 0) {
    lines.push('# Instances');
    for (const inst of doc.instances) {
      const tf = inst.transform ?? identityTransform();
      let line = `INST ${escapeId(inst.id)} ${escapeId(inst.partDefId)} ${formatQuotedString(inst.name ?? inst.id)} ${tf.translation.x} ${tf.translation.y} ${tf.translation.z} ${tf.rotation.x} ${tf.rotation.y} ${tf.rotation.z} ${tf.scale.x} ${tf.scale.y} ${tf.scale.z}`;
      if (inst.material) {
        line += ` ${escapeId(inst.material)}`;
      }
      lines.push(line);
    }
    lines.push('');
  }

  // Joints
  if (doc.joints && doc.joints.length > 0) {
    lines.push('# Joints');
    for (const joint of doc.joints) {
      lines.push(formatJoint(joint));
    }
    lines.push('');
  }

  // Ground
  if (doc.groundInstanceId) {
    lines.push(`GROUND ${escapeId(doc.groundInstanceId)}`);
    lines.push('');
  }

  // Scene settings
  if (doc.scene) {
    lines.push('# Scene Settings');
    formatSceneSettings(lines, doc.scene);
  }

  // Remove trailing empty lines
  while (lines.length > 0 && lines[lines.length - 1] === '') {
    lines.pop();
  }

  return lines.join('\n');
}

// ============================================================================
// VCode Helper Functions
// ============================================================================

/** Split a line by whitespace, but keep quoted strings together. */
function splitLineRespectingQuotes(line: string): string[] {
  const parts: string[] = [];
  let current = '';
  let inQuotes = false;

  for (let i = 0; i < line.length; i++) {
    const c = line[i];

    if (c === '"' && (i === 0 || line[i - 1] !== '\\')) {
      if (inQuotes) {
        current += c;
        parts.push(current);
        current = '';
        inQuotes = false;
      } else {
        if (current.trim()) {
          parts.push(...current.trim().split(/\s+/));
        }
        current = c;
        inQuotes = true;
      }
    } else if (inQuotes) {
      current += c;
    } else if (/\s/.test(c)) {
      if (current.trim()) {
        parts.push(current.trim());
      }
      current = '';
    } else {
      current += c;
    }
  }

  if (current.trim()) {
    parts.push(current.trim());
  }

  return parts;
}

/** Parse a potentially quoted string argument. */
function parseStringArg(s: string): string {
  if (s.startsWith('"') && s.endsWith('"') && s.length >= 2) {
    const inner = s.slice(1, -1);
    return inner.replace(/\\"/g, '"').replace(/\\\\/g, '\\');
  }
  return s;
}

/** Parse a decimal integer argument. Rejects hex/octal prefixes, NaN, and non-finite results. */
function parseIntArg(s: string, line: number, field?: string): number {
  const n = parseInt(s, 10);
  if (!Number.isFinite(n)) {
    throw new VCodeParseError(line, `expected integer${field ? ` for ${field}` : ''}, got ${JSON.stringify(s)}`);
  }
  return n;
}

/** Parse a finite floating-point argument. Rejects NaN and ±Infinity. */
function parseFloatArg(s: string, line: number, field?: string): number {
  const n = parseFloat(s);
  if (!Number.isFinite(n)) {
    throw new VCodeParseError(line, `expected finite number${field ? ` for ${field}` : ''}, got ${JSON.stringify(s)}`);
  }
  return n;
}

/** Escape an identifier - if it contains spaces or special chars, quote it. */
function escapeId(s: string): string {
  if (/[\s"]/.test(s) || s.length === 0 || /^\d/.test(s)) {
    return formatQuotedString(s);
  }
  return s;
}

/** Format a string with quotes. */
function formatQuotedString(s: string): string {
  return `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/** Get child node IDs from an operation. */
function getChildren(op: CsgOp): number[] {
  switch (op.type) {
    case 'Union':
    case 'Difference':
    case 'Intersection':
      return [op.left, op.right];
    case 'Translate':
    case 'Rotate':
    case 'Scale':
    case 'LinearPattern':
    case 'CircularPattern':
    case 'Shell':
    case 'Fillet':
    case 'Chamfer':
      return [op.child];
    case 'Extrude':
    case 'Revolve':
    case 'Sweep':
      return [op.sketch];
    case 'Loft':
      return op.sketches;
    case 'PcbBoard':
      return [];
    case 'EmbroideryPattern':
      return [];
    default:
      return [];
  }
}

/** Topological sort of nodes. */
function topologicalSort(doc: Document, roots: number[]): number[] {
  const result: number[] = [];
  const visited = new Set<number>();
  const tempVisited = new Set<number>();

  function visit(nodeId: number): void {
    if (visited.has(nodeId)) return;
    if (tempVisited.has(nodeId)) {
      throw new Error(`Cycle detected at node ${nodeId}`);
    }

    tempVisited.add(nodeId);

    const node = doc.nodes[nodeId.toString()];
    if (node) {
      for (const childId of getChildren(node.op)) {
        visit(childId);
      }
    }

    tempVisited.delete(nodeId);
    visited.add(nodeId);
    result.push(nodeId);
  }

  for (const rootId of roots) {
    visit(rootId);
  }

  // Also visit any orphan nodes
  for (const id of Object.keys(doc.nodes).map(Number)) {
    if (!visited.has(id)) {
      visit(id);
    }
  }

  return result;
}

/** Format a CsgOp as a VCode line. */
function formatOp(op: CsgOp, idMap: Map<number, number>, name?: string): string {
  const nameSuffix = name ? ` ${formatQuotedString(name)}` : '';

  switch (op.type) {
    case 'Cube':
      return `C ${op.size.x} ${op.size.y} ${op.size.z}${nameSuffix}`;
    case 'Cylinder':
      return `Y ${op.radius} ${op.height}${nameSuffix}`;
    case 'Sphere':
      return `S ${op.radius}${nameSuffix}`;
    case 'Cone':
      return `K ${op.radius_bottom} ${op.radius_top} ${op.height}${nameSuffix}`;
    case 'Torus':
      return `TO ${op.major_radius} ${op.minor_radius}${nameSuffix}`;
    case 'Wedge':
      return `W ${op.size.x} ${op.size.y} ${op.size.z}${nameSuffix}`;
    case 'Prism':
      return `PR ${op.sides} ${op.radius} ${op.height}${nameSuffix}`;
    case 'Mirror':
      return `MI ${idMap.get(op.child)} ${op.plane_origin.x} ${op.plane_origin.y} ${op.plane_origin.z} ${op.plane_normal.x} ${op.plane_normal.y} ${op.plane_normal.z}${nameSuffix}`;
    case 'Empty':
      return `C 0 0 0${nameSuffix}`;
    case 'Union':
      return `U ${idMap.get(op.left)} ${idMap.get(op.right)}${nameSuffix}`;
    case 'Difference':
      return `D ${idMap.get(op.left)} ${idMap.get(op.right)}${nameSuffix}`;
    case 'Intersection':
      return `I ${idMap.get(op.left)} ${idMap.get(op.right)}${nameSuffix}`;
    case 'Translate':
      return `T ${idMap.get(op.child)} ${op.offset.x} ${op.offset.y} ${op.offset.z}${nameSuffix}`;
    case 'Rotate':
      return `R ${idMap.get(op.child)} ${op.angles.x} ${op.angles.y} ${op.angles.z}${nameSuffix}`;
    case 'Scale':
      return `X ${idMap.get(op.child)} ${op.factor.x} ${op.factor.y} ${op.factor.z}${nameSuffix}`;
    case 'LinearPattern':
      return `LP ${idMap.get(op.child)} ${op.direction.x} ${op.direction.y} ${op.direction.z} ${op.count} ${op.spacing}${nameSuffix}`;
    case 'CircularPattern':
      return `CP ${idMap.get(op.child)} ${op.axis_origin.x} ${op.axis_origin.y} ${op.axis_origin.z} ${op.axis_dir.x} ${op.axis_dir.y} ${op.axis_dir.z} ${op.count} ${op.angle_deg}${nameSuffix}`;
    case 'Shell':
      return `SH ${idMap.get(op.child)} ${op.thickness}${nameSuffix}`;
    case 'Fillet':
      return `FI ${idMap.get(op.child)} ${op.radius}${nameSuffix}`;
    case 'Chamfer':
      return `CH ${idMap.get(op.child)} ${op.distance}${nameSuffix}`;
    case 'Sketch2D': {
      const skLines: string[] = [];
      skLines.push(`SK ${op.origin.x} ${op.origin.y} ${op.origin.z}  ${op.x_dir.x} ${op.x_dir.y} ${op.x_dir.z}  ${op.y_dir.x} ${op.y_dir.y} ${op.y_dir.z}${nameSuffix}`);
      for (const seg of op.segments) {
        if (seg.type === 'Line') {
          skLines.push(`L ${seg.start.x} ${seg.start.y} ${seg.end.x} ${seg.end.y}`);
        } else {
          skLines.push(`A ${seg.start.x} ${seg.start.y} ${seg.end.x} ${seg.end.y} ${seg.center.x} ${seg.center.y} ${seg.ccw ? 1 : 0}`);
        }
      }
      skLines.push('END');
      return skLines.join('\n');
    }
    case 'Extrude':
      return `E ${idMap.get(op.sketch)} ${op.direction.x} ${op.direction.y} ${op.direction.z}${nameSuffix}`;
    case 'Revolve':
      return `V ${idMap.get(op.sketch)} ${op.axis_origin.x} ${op.axis_origin.y} ${op.axis_origin.z} ${op.axis_dir.x} ${op.axis_dir.y} ${op.axis_dir.z} ${op.angle_deg}${nameSuffix}`;
    case 'PcbBoard':
      throw new Error('PcbBoard not supported in VCode');
    case 'EmbroideryPattern':
      throw new Error('EmbroideryPattern not supported in VCode');
    default:
      throw new Error(`Unsupported op type for VCode: ${(op as CsgOp).type}`);
  }
}

/** Format a joint. */
function formatJoint(joint: Joint): string {
  const parent = joint.parentInstanceId ? escapeId(joint.parentInstanceId) : '_';
  const child = escapeId(joint.childInstanceId);
  const pa = joint.parentAnchor;
  const ca = joint.childAnchor;

  switch (joint.kind.type) {
    case 'Fixed':
      return `JFIX ${escapeId(joint.id)} ${parent} ${child} ${pa.x} ${pa.y} ${pa.z} ${ca.x} ${ca.y} ${ca.z}`;
    case 'Revolute': {
      let line = `JREV ${escapeId(joint.id)} ${parent} ${child} ${pa.x} ${pa.y} ${pa.z} ${ca.x} ${ca.y} ${ca.z} ${joint.kind.axis.x} ${joint.kind.axis.y} ${joint.kind.axis.z}`;
      if (joint.kind.limits) {
        line += ` ${joint.kind.limits[0]} ${joint.kind.limits[1]}`;
      }
      return line;
    }
    case 'Slider': {
      let line = `JSLD ${escapeId(joint.id)} ${parent} ${child} ${pa.x} ${pa.y} ${pa.z} ${ca.x} ${ca.y} ${ca.z} ${joint.kind.axis.x} ${joint.kind.axis.y} ${joint.kind.axis.z}`;
      if (joint.kind.limits) {
        line += ` ${joint.kind.limits[0]} ${joint.kind.limits[1]}`;
      }
      return line;
    }
    case 'Cylindrical':
      return `JCYL ${escapeId(joint.id)} ${parent} ${child} ${pa.x} ${pa.y} ${pa.z} ${ca.x} ${ca.y} ${ca.z} ${joint.kind.axis.x} ${joint.kind.axis.y} ${joint.kind.axis.z}`;
    case 'Ball':
      return `JBAL ${escapeId(joint.id)} ${parent} ${child} ${pa.x} ${pa.y} ${pa.z} ${ca.x} ${ca.y} ${ca.z}`;
    case 'Free':
      return `JFRE ${escapeId(joint.id)} ${parent} ${child} ${pa.x} ${pa.y} ${pa.z} ${ca.x} ${ca.y} ${ca.z}`;
  }
}

/** Format scene settings. */
function formatSceneSettings(lines: string[], scene: SceneSettings): void {
  if (scene.environment) {
    if (scene.environment.type === 'None') {
      lines.push('ENV none');
    } else if (scene.environment.type === 'Preset') {
      lines.push(`ENV ${scene.environment.preset} ${scene.environment.intensity ?? 1.0}`);
    } else {
      lines.push(`ENV ${formatQuotedString(scene.environment.url)} ${scene.environment.intensity ?? 1.0}`);
    }
  }

  if (scene.background) {
    switch (scene.background.type) {
      case 'Solid':
        lines.push(`BG solid ${scene.background.color[0]} ${scene.background.color[1]} ${scene.background.color[2]}`);
        break;
      case 'Gradient':
        lines.push(`BG gradient ${scene.background.top[0]} ${scene.background.top[1]} ${scene.background.top[2]} ${scene.background.bottom[0]} ${scene.background.bottom[1]} ${scene.background.bottom[2]}`);
        break;
      case 'Environment':
        lines.push('BG env');
        break;
      case 'Transparent':
        lines.push('BG transparent');
        break;
    }
  }

  if (scene.lights) {
    for (const light of scene.lights) {
      const c = light.color;
      const i = light.intensity;
      const shadow = light.castShadow ? ' shadow' : '';

      switch (light.kind.type) {
        case 'Directional':
          lines.push(`LDIR ${escapeId(light.id)} ${c[0]} ${c[1]} ${c[2]} ${i} ${light.kind.direction.x} ${light.kind.direction.y} ${light.kind.direction.z}${shadow}`);
          break;
        case 'Point': {
          let line = `LPNT ${escapeId(light.id)} ${c[0]} ${c[1]} ${c[2]} ${i} ${light.kind.position.x} ${light.kind.position.y} ${light.kind.position.z}`;
          if (light.kind.distance !== undefined) {
            line += ` ${light.kind.distance}`;
          }
          lines.push(line);
          break;
        }
        case 'Spot': {
          let line = `LSPT ${escapeId(light.id)} ${c[0]} ${c[1]} ${c[2]} ${i} ${light.kind.position.x} ${light.kind.position.y} ${light.kind.position.z} ${light.kind.direction.x} ${light.kind.direction.y} ${light.kind.direction.z}`;
          if (light.kind.angle !== undefined) {
            line += ` ${light.kind.angle}`;
            if (light.kind.penumbra !== undefined) {
              line += ` ${light.kind.penumbra}`;
            }
          }
          lines.push(line);
          break;
        }
        case 'Area':
          lines.push(`LAREA ${escapeId(light.id)} ${c[0]} ${c[1]} ${c[2]} ${i} ${light.kind.position.x} ${light.kind.position.y} ${light.kind.position.z} ${light.kind.direction.x} ${light.kind.direction.y} ${light.kind.direction.z} ${light.kind.width} ${light.kind.height}`);
          break;
      }
    }
  }

  if (scene.postProcessing) {
    const pp = scene.postProcessing;
    if (pp.ambientOcclusion) {
      lines.push(`AO ${pp.ambientOcclusion.enabled ? 1 : 0} ${pp.ambientOcclusion.intensity ?? 1.0} ${pp.ambientOcclusion.radius ?? 1.0}`);
    }
    if (pp.bloom) {
      lines.push(`BLOOM ${pp.bloom.enabled ? 1 : 0} ${pp.bloom.intensity ?? 0.5} ${pp.bloom.threshold ?? 0.8}`);
    }
    if (pp.vignette) {
      lines.push(`VIG ${pp.vignette.enabled ? 1 : 0} ${pp.vignette.offset ?? 0.3} ${pp.vignette.darkness ?? 0.5}`);
    }
    if (pp.toneMapping) {
      lines.push(`TONE ${pp.toneMapping}`);
    }
    if (pp.exposure !== undefined) {
      lines.push(`EXP ${pp.exposure}`);
    }
  }

  if (scene.cameraPresets) {
    for (const cam of scene.cameraPresets) {
      let line = `CAM ${escapeId(cam.id)} ${cam.position.x} ${cam.position.y} ${cam.position.z} ${cam.target.x} ${cam.target.y} ${cam.target.z}`;
      if (cam.fov !== undefined) {
        line += ` ${cam.fov}`;
      }
      if (cam.name) {
        line += ` ${formatQuotedString(cam.name)}`;
      }
      lines.push(line);
    }
  }
}

// ============================================================================
// VCode Parsing Functions
// ============================================================================

function parseMaterial(doc: Document, parts: string[], line: number): void {
  if (parts.length < 7) {
    throw new VCodeParseError(line, `M requires at least 6 args, got ${parts.length - 1}`);
  }

  const name = parseStringArg(parts[1]);
  const color: [number, number, number] = [
    parseFloatArg(parts[2], line, 'color.r'),
    parseFloatArg(parts[3], line, 'color.g'),
    parseFloatArg(parts[4], line, 'color.b'),
  ];
  const metallic = parseFloatArg(parts[5], line, 'metallic');
  const roughness = parseFloatArg(parts[6], line, 'roughness');
  const density = parts[7] ? parseFloatArg(parts[7], line, 'density') : undefined;
  const friction = parts[8] ? parseFloatArg(parts[8], line, 'friction') : undefined;

  doc.materials[name] = { name, color, metallic, roughness, density, friction };
}

function parseRoot(doc: Document, parts: string[], line: number): void {
  if (parts.length < 3) {
    throw new VCodeParseError(line, `ROOT requires at least 2 args, got ${parts.length - 1}`);
  }

  const root = parseIntArg(parts[1], line, 'root');
  const material = parseStringArg(parts[2]);
  const visible = parts[3] === 'hidden' ? false : undefined;

  doc.roots.push({ root, material, visible });
}

function parsePartDef(doc: Document, parts: string[], line: number): void {
  if (parts.length < 4) {
    throw new VCodeParseError(line, `PDEF requires at least 3 args, got ${parts.length - 1}`);
  }

  const id = parseStringArg(parts[1]);
  const name = parseStringArg(parts[2]);
  const root = parseIntArg(parts[3], line, 'root');
  const defaultMaterial = parts[4] ? parseStringArg(parts[4]) : undefined;

  if (!doc.partDefs) doc.partDefs = {};
  doc.partDefs[id] = { id, name, root, defaultMaterial };
}

function parseInstance(doc: Document, parts: string[], line: number): void {
  if (parts.length < 13) {
    throw new VCodeParseError(line, `INST requires at least 12 args, got ${parts.length - 1}`);
  }

  const id = parseStringArg(parts[1]);
  const partDefId = parseStringArg(parts[2]);
  const name = parseStringArg(parts[3]);
  const transform: Transform3D = {
    translation: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
    rotation: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
    scale: { x: parseFloat(parts[10]), y: parseFloat(parts[11]), z: parseFloat(parts[12]) },
  };
  const material = parts[13] ? parseStringArg(parts[13]) : undefined;

  if (!doc.instances) doc.instances = [];
  doc.instances.push({ id, partDefId, name, transform, material });
}

function parseJoint(doc: Document, opcode: string, parts: string[], line: number): void {
  if (!doc.joints) doc.joints = [];

  const parseOptionalParent = (s: string): string | null => s === '_' ? null : parseStringArg(s);

  switch (opcode) {
    case 'JFIX': {
      if (parts.length < 10) {
        throw new VCodeParseError(line, `JFIX requires 9 args, got ${parts.length - 1}`);
      }
      doc.joints.push({
        id: parseStringArg(parts[1]),
        parentInstanceId: parseOptionalParent(parts[2]),
        childInstanceId: parseStringArg(parts[3]),
        parentAnchor: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
        childAnchor: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
        kind: { type: 'Fixed' },
        state: 0,
      });
      break;
    }
    case 'JREV': {
      if (parts.length < 13) {
        throw new VCodeParseError(line, `JREV requires at least 12 args, got ${parts.length - 1}`);
      }
      const limits: [number, number] | undefined = parts.length >= 15
        ? [parseFloat(parts[13]), parseFloat(parts[14])]
        : undefined;
      doc.joints.push({
        id: parseStringArg(parts[1]),
        parentInstanceId: parseOptionalParent(parts[2]),
        childInstanceId: parseStringArg(parts[3]),
        parentAnchor: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
        childAnchor: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
        kind: {
          type: 'Revolute',
          axis: { x: parseFloat(parts[10]), y: parseFloat(parts[11]), z: parseFloat(parts[12]) },
          limits,
        },
        state: 0,
      });
      break;
    }
    case 'JSLD': {
      if (parts.length < 13) {
        throw new VCodeParseError(line, `JSLD requires at least 12 args, got ${parts.length - 1}`);
      }
      const limits: [number, number] | undefined = parts.length >= 15
        ? [parseFloat(parts[13]), parseFloat(parts[14])]
        : undefined;
      doc.joints.push({
        id: parseStringArg(parts[1]),
        parentInstanceId: parseOptionalParent(parts[2]),
        childInstanceId: parseStringArg(parts[3]),
        parentAnchor: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
        childAnchor: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
        kind: {
          type: 'Slider',
          axis: { x: parseFloat(parts[10]), y: parseFloat(parts[11]), z: parseFloat(parts[12]) },
          limits,
        },
        state: 0,
      });
      break;
    }
    case 'JCYL': {
      if (parts.length < 13) {
        throw new VCodeParseError(line, `JCYL requires 12 args, got ${parts.length - 1}`);
      }
      doc.joints.push({
        id: parseStringArg(parts[1]),
        parentInstanceId: parseOptionalParent(parts[2]),
        childInstanceId: parseStringArg(parts[3]),
        parentAnchor: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
        childAnchor: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
        kind: {
          type: 'Cylindrical',
          axis: { x: parseFloat(parts[10]), y: parseFloat(parts[11]), z: parseFloat(parts[12]) },
        },
        state: 0,
      });
      break;
    }
    case 'JBAL': {
      if (parts.length < 10) {
        throw new VCodeParseError(line, `JBAL requires 9 args, got ${parts.length - 1}`);
      }
      doc.joints.push({
        id: parseStringArg(parts[1]),
        parentInstanceId: parseOptionalParent(parts[2]),
        childInstanceId: parseStringArg(parts[3]),
        parentAnchor: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
        childAnchor: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
        kind: { type: 'Ball' },
        state: 0,
      });
      break;
    }
    case 'JFRE': {
      if (parts.length < 10) {
        throw new VCodeParseError(line, `JFRE requires 9 args, got ${parts.length - 1}`);
      }
      doc.joints.push({
        id: parseStringArg(parts[1]),
        parentInstanceId: parseOptionalParent(parts[2]),
        childInstanceId: parseStringArg(parts[3]),
        parentAnchor: { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) },
        childAnchor: { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) },
        kind: { type: 'Free' },
        state: 0,
      });
      break;
    }
  }
}

function parseEnvironment(doc: Document, parts: string[], line: number): void {
  if (parts.length < 2) {
    throw new VCodeParseError(line, `ENV requires at least 1 arg, got ${parts.length - 1}`);
  }

  if (!doc.scene) doc.scene = {};
  const presetOrUrl = parseStringArg(parts[1]);

  if (presetOrUrl === 'none') {
    doc.scene.environment = { type: 'None' };
    return;
  }

  if (parts.length < 3) {
    throw new VCodeParseError(line, `ENV requires 2 args, got ${parts.length - 1}`);
  }

  const intensity = parseFloat(parts[2]);

  const presets = ['studio', 'warehouse', 'apartment', 'park', 'city', 'dawn', 'night', 'sunset', 'forest', 'neutral'];
  if (presets.includes(presetOrUrl)) {
    doc.scene.environment = { type: 'Preset', preset: presetOrUrl as EnvironmentPreset, intensity };
  } else {
    doc.scene.environment = { type: 'Custom', url: presetOrUrl, intensity };
  }
}

function parseBackground(doc: Document, parts: string[], line: number): void {
  if (parts.length < 2) {
    throw new VCodeParseError(line, 'BG requires at least 1 arg');
  }

  if (!doc.scene) doc.scene = {};

  switch (parts[1]) {
    case 'solid':
      if (parts.length < 5) {
        throw new VCodeParseError(line, 'BG solid requires 3 color values');
      }
      doc.scene.background = { type: 'Solid', color: [parseFloat(parts[2]), parseFloat(parts[3]), parseFloat(parts[4])] };
      break;
    case 'gradient':
      if (parts.length < 8) {
        throw new VCodeParseError(line, 'BG gradient requires 6 color values');
      }
      doc.scene.background = {
        type: 'Gradient',
        top: [parseFloat(parts[2]), parseFloat(parts[3]), parseFloat(parts[4])],
        bottom: [parseFloat(parts[5]), parseFloat(parts[6]), parseFloat(parts[7])],
      };
      break;
    case 'env':
      doc.scene.background = { type: 'Environment' };
      break;
    case 'transparent':
      doc.scene.background = { type: 'Transparent' };
      break;
    default:
      throw new VCodeParseError(line, `Unknown BG type: ${parts[1]}`);
  }
}

function parseLight(doc: Document, opcode: string, parts: string[], line: number): void {
  if (!doc.scene) doc.scene = {};
  if (!doc.scene.lights) doc.scene.lights = [];

  switch (opcode) {
    case 'LDIR': {
      if (parts.length < 9) {
        throw new VCodeParseError(line, `LDIR requires at least 8 args, got ${parts.length - 1}`);
      }
      const castShadow = parts[9] === 'shadow';
      doc.scene.lights.push({
        id: parseStringArg(parts[1]),
        color: [parseFloat(parts[2]), parseFloat(parts[3]), parseFloat(parts[4])],
        intensity: parseFloat(parts[5]),
        kind: { type: 'Directional', direction: { x: parseFloat(parts[6]), y: parseFloat(parts[7]), z: parseFloat(parts[8]) } },
        enabled: true,
        castShadow: castShadow || undefined,
      });
      break;
    }
    case 'LPNT': {
      if (parts.length < 9) {
        throw new VCodeParseError(line, `LPNT requires at least 8 args, got ${parts.length - 1}`);
      }
      const distance = parts[9] ? parseFloat(parts[9]) : undefined;
      doc.scene.lights.push({
        id: parseStringArg(parts[1]),
        color: [parseFloat(parts[2]), parseFloat(parts[3]), parseFloat(parts[4])],
        intensity: parseFloat(parts[5]),
        kind: { type: 'Point', position: { x: parseFloat(parts[6]), y: parseFloat(parts[7]), z: parseFloat(parts[8]) }, distance },
        enabled: true,
      });
      break;
    }
    case 'LSPT': {
      if (parts.length < 12) {
        throw new VCodeParseError(line, `LSPT requires at least 11 args, got ${parts.length - 1}`);
      }
      const angle = parts[12] ? parseFloat(parts[12]) : undefined;
      const penumbra = parts[13] ? parseFloat(parts[13]) : undefined;
      doc.scene.lights.push({
        id: parseStringArg(parts[1]),
        color: [parseFloat(parts[2]), parseFloat(parts[3]), parseFloat(parts[4])],
        intensity: parseFloat(parts[5]),
        kind: {
          type: 'Spot',
          position: { x: parseFloat(parts[6]), y: parseFloat(parts[7]), z: parseFloat(parts[8]) },
          direction: { x: parseFloat(parts[9]), y: parseFloat(parts[10]), z: parseFloat(parts[11]) },
          angle,
          penumbra,
        },
        enabled: true,
      });
      break;
    }
    case 'LAREA': {
      if (parts.length < 14) {
        throw new VCodeParseError(line, `LAREA requires 13 args, got ${parts.length - 1}`);
      }
      doc.scene.lights.push({
        id: parseStringArg(parts[1]),
        color: [parseFloat(parts[2]), parseFloat(parts[3]), parseFloat(parts[4])],
        intensity: parseFloat(parts[5]),
        kind: {
          type: 'Area',
          position: { x: parseFloat(parts[6]), y: parseFloat(parts[7]), z: parseFloat(parts[8]) },
          direction: { x: parseFloat(parts[9]), y: parseFloat(parts[10]), z: parseFloat(parts[11]) },
          width: parseFloat(parts[12]),
          height: parseFloat(parts[13]),
        },
        enabled: true,
      });
      break;
    }
  }
}

function parseAO(doc: Document, parts: string[], line: number): void {
  if (parts.length < 4) {
    throw new VCodeParseError(line, `AO requires 3 args, got ${parts.length - 1}`);
  }

  if (!doc.scene) doc.scene = {};
  if (!doc.scene.postProcessing) doc.scene.postProcessing = {};

  doc.scene.postProcessing.ambientOcclusion = {
    enabled: parseInt(parts[1], 10) !== 0,
    intensity: parseFloat(parts[2]),
    radius: parseFloat(parts[3]),
  };
}

function parseBloom(doc: Document, parts: string[], line: number): void {
  if (parts.length < 4) {
    throw new VCodeParseError(line, `BLOOM requires 3 args, got ${parts.length - 1}`);
  }

  if (!doc.scene) doc.scene = {};
  if (!doc.scene.postProcessing) doc.scene.postProcessing = {};

  doc.scene.postProcessing.bloom = {
    enabled: parseInt(parts[1], 10) !== 0,
    intensity: parseFloat(parts[2]),
    threshold: parseFloat(parts[3]),
  };
}

function parseVignette(doc: Document, parts: string[], line: number): void {
  if (parts.length < 4) {
    throw new VCodeParseError(line, `VIG requires 3 args, got ${parts.length - 1}`);
  }

  if (!doc.scene) doc.scene = {};
  if (!doc.scene.postProcessing) doc.scene.postProcessing = {};

  doc.scene.postProcessing.vignette = {
    enabled: parseInt(parts[1], 10) !== 0,
    offset: parseFloat(parts[2]),
    darkness: parseFloat(parts[3]),
  };
}

function parseToneMapping(doc: Document, parts: string[], line: number): void {
  if (parts.length < 2) {
    throw new VCodeParseError(line, 'TONE requires 1 arg');
  }

  if (!doc.scene) doc.scene = {};
  if (!doc.scene.postProcessing) doc.scene.postProcessing = {};

  const validMappings = ['none', 'reinhard', 'cineon', 'acesFilmic', 'agX', 'neutral'];
  if (!validMappings.includes(parts[1])) {
    throw new VCodeParseError(line, `Unknown tone mapping: ${parts[1]}`);
  }
  doc.scene.postProcessing.toneMapping = parts[1] as ToneMapping;
}

function parseExposure(doc: Document, parts: string[], line: number): void {
  if (parts.length < 2) {
    throw new VCodeParseError(line, 'EXP requires 1 arg');
  }

  if (!doc.scene) doc.scene = {};
  if (!doc.scene.postProcessing) doc.scene.postProcessing = {};

  doc.scene.postProcessing.exposure = parseFloat(parts[1]);
}

function parseCamera(doc: Document, parts: string[], line: number): void {
  if (parts.length < 8) {
    throw new VCodeParseError(line, `CAM requires at least 7 args, got ${parts.length - 1}`);
  }

  if (!doc.scene) doc.scene = {};
  if (!doc.scene.cameraPresets) doc.scene.cameraPresets = [];

  const fov = parts[8] ? parseFloat(parts[8]) : undefined;
  const name = parts[9] ? parseStringArg(parts[9]) : undefined;

  doc.scene.cameraPresets.push({
    id: parseStringArg(parts[1]),
    position: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) },
    target: { x: parseFloat(parts[5]), y: parseFloat(parts[6]), z: parseFloat(parts[7]) },
    fov,
    name,
  });
}

/** Parse a geometry line. Returns [op, name, newLineIndex]. */
function parseGeometryLine(line: string, lineNum: number, lines: string[]): [CsgOp, string | undefined, number] {
  const parts = splitLineRespectingQuotes(line);
  if (parts.length === 0) {
    throw new VCodeParseError(lineNum, 'empty line');
  }

  const opcode = parts[0];

  // Extract trailing quoted name if present
  let name: string | undefined;
  let args = parts;
  const last = parts[parts.length - 1];
  if (last.startsWith('"') && last.endsWith('"')) {
    name = parseStringArg(last);
    args = parts.slice(0, -1);
  }

  const op = parseGeometryOpcode(opcode, args, lineNum, lines);
  const newIndex = opcode === 'SK' ? findSketchEndLine(lineNum, lines) : lineNum;

  return [op, name, newIndex];
}

/** Find the END line for a sketch. */
function findSketchEndLine(startLine: number, lines: string[]): number {
  for (let i = startLine + 1; i < lines.length; i++) {
    if (lines[i].trim() === 'END') {
      return i;
    }
  }
  return startLine;
}

/** Parse a geometry opcode. */
function parseGeometryOpcode(opcode: string, parts: string[], lineNum: number, lines: string[]): CsgOp {
  switch (opcode) {
    case 'C':
      if (parts.length !== 4) throw new VCodeParseError(lineNum, `C requires 3 args, got ${parts.length - 1}`);
      return { type: 'Cube', size: { x: parseFloat(parts[1]), y: parseFloat(parts[2]), z: parseFloat(parts[3]) } };

    case 'Y':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `Y requires 2 args, got ${parts.length - 1}`);
      return { type: 'Cylinder', radius: parseFloat(parts[1]), height: parseFloat(parts[2]), segments: 0 };

    case 'S':
      if (parts.length !== 2) throw new VCodeParseError(lineNum, `S requires 1 arg, got ${parts.length - 1}`);
      return { type: 'Sphere', radius: parseFloat(parts[1]), segments: 0 };

    case 'K':
      if (parts.length !== 4) throw new VCodeParseError(lineNum, `K requires 3 args, got ${parts.length - 1}`);
      return { type: 'Cone', radius_bottom: parseFloat(parts[1]), radius_top: parseFloat(parts[2]), height: parseFloat(parts[3]), segments: 0 };

    case 'TO':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `TO requires 2 args, got ${parts.length - 1}`);
      return { type: 'Torus', major_radius: parseFloatArg(parts[1], lineNum, 'major_radius'), minor_radius: parseFloatArg(parts[2], lineNum, 'minor_radius'), segments: 0 };

    case 'W':
      if (parts.length !== 4) throw new VCodeParseError(lineNum, `W requires 3 args, got ${parts.length - 1}`);
      return { type: 'Wedge', size: { x: parseFloatArg(parts[1], lineNum, 'sx'), y: parseFloatArg(parts[2], lineNum, 'sy'), z: parseFloatArg(parts[3], lineNum, 'sz') } };

    case 'PR':
      if (parts.length !== 4) throw new VCodeParseError(lineNum, `PR requires 3 args, got ${parts.length - 1}`);
      return { type: 'Prism', sides: parseIntArg(parts[1], lineNum, 'sides'), radius: parseFloatArg(parts[2], lineNum, 'radius'), height: parseFloatArg(parts[3], lineNum, 'height') };

    case 'MI':
      if (parts.length !== 8) throw new VCodeParseError(lineNum, `MI requires 7 args, got ${parts.length - 1}`);
      return { type: 'Mirror', child: parseIntArg(parts[1], lineNum, 'child'), plane_origin: { x: parseFloatArg(parts[2], lineNum, 'ox'), y: parseFloatArg(parts[3], lineNum, 'oy'), z: parseFloatArg(parts[4], lineNum, 'oz') }, plane_normal: { x: parseFloatArg(parts[5], lineNum, 'nx'), y: parseFloatArg(parts[6], lineNum, 'ny'), z: parseFloatArg(parts[7], lineNum, 'nz') } };

    case 'U':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `U requires 2 args, got ${parts.length - 1}`);
      return { type: 'Union', left: parseInt(parts[1], 10), right: parseInt(parts[2], 10) };

    case 'D':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `D requires 2 args, got ${parts.length - 1}`);
      return { type: 'Difference', left: parseInt(parts[1], 10), right: parseInt(parts[2], 10) };

    case 'I':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `I requires 2 args, got ${parts.length - 1}`);
      return { type: 'Intersection', left: parseInt(parts[1], 10), right: parseInt(parts[2], 10) };

    case 'T':
      if (parts.length !== 5) throw new VCodeParseError(lineNum, `T requires 4 args, got ${parts.length - 1}`);
      return { type: 'Translate', child: parseInt(parts[1], 10), offset: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) } };

    case 'R':
      if (parts.length !== 5) throw new VCodeParseError(lineNum, `R requires 4 args, got ${parts.length - 1}`);
      return { type: 'Rotate', child: parseInt(parts[1], 10), angles: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) } };

    case 'X':
      if (parts.length !== 5) throw new VCodeParseError(lineNum, `X requires 4 args, got ${parts.length - 1}`);
      return { type: 'Scale', child: parseInt(parts[1], 10), factor: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) } };

    case 'LP':
      if (parts.length !== 7) throw new VCodeParseError(lineNum, `LP requires 6 args, got ${parts.length - 1}`);
      return { type: 'LinearPattern', child: parseInt(parts[1], 10), direction: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) }, count: parseInt(parts[5], 10), spacing: parseFloat(parts[6]) };

    case 'CP':
      if (parts.length !== 10) throw new VCodeParseError(lineNum, `CP requires 9 args, got ${parts.length - 1}`);
      return { type: 'CircularPattern', child: parseInt(parts[1], 10), axis_origin: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) }, axis_dir: { x: parseFloat(parts[5]), y: parseFloat(parts[6]), z: parseFloat(parts[7]) }, count: parseInt(parts[8], 10), angle_deg: parseFloat(parts[9]) };

    case 'SH':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `SH requires 2 args, got ${parts.length - 1}`);
      return { type: 'Shell', child: parseInt(parts[1], 10), thickness: parseFloat(parts[2]) };

    case 'FI':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `FI requires 2 args, got ${parts.length - 1}`);
      return { type: 'Fillet', child: parseInt(parts[1], 10), radius: parseFloat(parts[2]) };

    case 'CH':
      if (parts.length !== 3) throw new VCodeParseError(lineNum, `CH requires 2 args, got ${parts.length - 1}`);
      return { type: 'Chamfer', child: parseInt(parts[1], 10), distance: parseFloat(parts[2]) };

    case 'SK': {
      if (parts.length !== 10) throw new VCodeParseError(lineNum, `SK requires 9 args, got ${parts.length - 1}`);
      const origin = { x: parseFloat(parts[1]), y: parseFloat(parts[2]), z: parseFloat(parts[3]) };
      const x_dir = { x: parseFloat(parts[4]), y: parseFloat(parts[5]), z: parseFloat(parts[6]) };
      const y_dir = { x: parseFloat(parts[7]), y: parseFloat(parts[8]), z: parseFloat(parts[9]) };
      const segments: SketchSegment2D[] = [];

      for (let idx = lineNum + 1; idx < lines.length; idx++) {
        const segLine = lines[idx].trim();
        if (segLine === 'END') break;
        if (!segLine || segLine.startsWith('#')) continue;

        const segParts = segLine.split(/\s+/);
        if (segParts[0] === 'L') {
          if (segParts.length !== 5) throw new VCodeParseError(idx, `L requires 4 args, got ${segParts.length - 1}`);
          segments.push({
            type: 'Line',
            start: { x: parseFloat(segParts[1]), y: parseFloat(segParts[2]) },
            end: { x: parseFloat(segParts[3]), y: parseFloat(segParts[4]) },
          });
        } else if (segParts[0] === 'A') {
          if (segParts.length !== 8) throw new VCodeParseError(idx, `A requires 7 args, got ${segParts.length - 1}`);
          segments.push({
            type: 'Arc',
            start: { x: parseFloat(segParts[1]), y: parseFloat(segParts[2]) },
            end: { x: parseFloat(segParts[3]), y: parseFloat(segParts[4]) },
            center: { x: parseFloat(segParts[5]), y: parseFloat(segParts[6]) },
            ccw: parseInt(segParts[7]) !== 0,
          });
        } else {
          throw new VCodeParseError(idx, `Unknown sketch segment opcode: ${segParts[0]}`);
        }
      }

      return { type: 'Sketch2D', origin, x_dir, y_dir, segments };
    }

    case 'E':
      if (parts.length !== 5) throw new VCodeParseError(lineNum, `E requires 4 args, got ${parts.length - 1}`);
      return { type: 'Extrude', sketch: parseInt(parts[1], 10), direction: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) } };

    case 'V':
      if (parts.length !== 9) throw new VCodeParseError(lineNum, `V requires 8 args, got ${parts.length - 1}`);
      return { type: 'Revolve', sketch: parseInt(parts[1], 10), axis_origin: { x: parseFloat(parts[2]), y: parseFloat(parts[3]), z: parseFloat(parts[4]) }, axis_dir: { x: parseFloat(parts[5]), y: parseFloat(parts[6]), z: parseFloat(parts[7]) }, angle_deg: parseFloat(parts[8]) };

    default:
      throw new VCodeParseError(lineNum, `Unknown opcode: ${opcode}`);
  }
}

/** Error thrown when parsing VCode fails. */
export class VCodeParseError extends Error {
  line: number;

  constructor(line: number, message: string) {
    super(`line ${line}: ${message}`);
    this.name = 'VCodeParseError';
    this.line = line;
  }
}
