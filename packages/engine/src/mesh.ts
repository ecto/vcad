/** Triangle mesh output — positions, indices, and optional normals for rendering. */
export interface TriangleMesh {
  positions: Float32Array;
  indices: Uint32Array;
  /** Optional vertex normals for smooth shading. If undefined, renderer computes them. */
  normals?: Float32Array;
  /** Optional per-vertex RGB colors (3 floats per vertex, 0–1 range). */
  colors?: Float32Array;
  /** Optional per-triangle face-kind tag (one u8 per `indices / 3`).
   *  Values: 0=Unknown, 1=Plane, 2=Cylinder, 3=Sphere, 4=Cone,
   *  5=Bilinear, 6=Torus, 7=BSpline, 8=FanFill. Used by the viewport
   *  click-to-inspect debugger. */
  faceKinds?: Uint8Array;
}

/** A single evaluated part with its mesh and material key. */
export interface EvaluatedPart {
  mesh: TriangleMesh;
  material: string;
  /** Optional BRep solid for ray tracing (only available for primitives, not boolean results). */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  solid?: any;
}

/** A part definition in an assembly (reusable geometry). */
export interface EvaluatedPartDef {
  id: string;
  mesh: TriangleMesh;
  /** Optional BRep solid handle (only set on the main-thread TS evaluator
   *  via `evaluateWithSolids` — used by direct-BRep ray tracing to upload
   *  per-instance geometry). Worker-evaluated scenes don't carry handles. */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  solid?: any;
}

/** An instance of a part definition with transform and material. */
export interface EvaluatedInstance {
  instanceId: string;
  partDefId: string;
  name?: string;
  mesh: TriangleMesh;
  material: string;
  transform?: {
    translation: { x: number; y: number; z: number };
    rotation: { x: number; y: number; z: number };
    scale: { x: number; y: number; z: number };
  };
}

/** Per-node timing from the kernel evaluator. */
export interface NodeTimingData {
  /** Operation name (e.g. "Sweep", "Union"). */
  op: string;
  /** Kernel operation time (ms). */
  eval_ms: number;
  /** Tessellation time for this node (ms). */
  mesh_ms: number;
}

/** Timing breakdown for a document evaluation. */
export interface EvalTimingData {
  /** Total evaluation time inside the kernel (ms). */
  total_ms: number;
  /** JSON parse time at WASM boundary (ms). */
  parse_ms?: number;
  /** serde_wasm_bindgen serialization time (ms). */
  serialize_ms?: number;
  /** Total tessellation time across all nodes (ms). */
  tessellate_ms: number;
  /** Clash detection time (ms). */
  clash_ms: number;
  /** Assembly evaluation time (ms). */
  assembly_ms: number;
  /** Per-node timing keyed by node ID. */
  nodes: Record<string, NodeTimingData>;
}

/** A single feature that failed to evaluate. Evaluation is per-root, so one
 * bad feature produces an empty mesh + a RootFailure instead of aborting the
 * whole scene. */
export interface RootFailure {
  /** Where the failure happened: `"root[<idx>]"` or `"partDef[\"<id>\"]"`. */
  scope: string;
  /** Root node id that failed to evaluate. */
  node_id: number;
  /** Human-readable error message. */
  error: string;
}

/** Result of evaluating a full document — one part per scene root. */
export interface EvaluatedScene {
  parts: EvaluatedPart[];
  /** Part definitions for assembly mode. */
  partDefs?: EvaluatedPartDef[];
  /** Instances for assembly mode. */
  instances?: EvaluatedInstance[];
  /** Meshes representing intersections between overlapping parts (for clash visualization). */
  clashes: TriangleMesh[];
  /** Per-root evaluation failures. Absent/empty on a fully successful eval. */
  failures?: RootFailure[];
  /** Timing breakdown (present when WASM evaluator provides it). */
  timing?: EvalTimingData;
}
