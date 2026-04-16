import type { Document, Vec3, SketchSegment2D, NodeId } from "@vcad/ir";
import { evaluateDocument, type EvaluateOptions } from "./evaluate.js";
import type { EvaluatedScene, TriangleMesh } from "./mesh.js";
import type { Solid, WasmAnnotationLayer } from "@vcad/kernel-wasm";
import { SolidCache } from "./solid-cache.js";
import { MeshCache } from "./mesh-cache.js";
import { DependencyGraph } from "./dependency-graph.js";

export type {
  TriangleMesh,
  EvaluatedPart,
  EvaluatedPartDef,
  EvaluatedInstance,
  EvaluatedScene,
  EvalTimingData,
  NodeTimingData,
} from "./mesh.js";

export {
  solveForwardKinematics,
  applyForwardKinematics,
} from "./kinematics.js";

export {
  getKernelWasm,
  getKernelWasmSync,
  primeKernelWasm,
} from "./wasm-singleton.js";

export {
  initializeGpu,
  isGpuAvailable,
  processGeometryGpu,
  computeCreasedNormalsGpu,
  decimateMeshGpu,
  mergeMeshes,
  initializeRayTracer,
  getRayTracer,
  isRayTracerAvailable,
} from "./gpu.js";

export type { GpuGeometryResult } from "./gpu.js";

// Caching and incremental evaluation
export { SolidCache, hashCsgOp } from "./solid-cache.js";
export { MeshCache } from "./mesh-cache.js";
export { DependencyGraph } from "./dependency-graph.js";
export type { EvaluateOptions } from "./evaluate.js";

// ECAD (Electronics)
export {
  isEcadAvailable,
  runDrc,
  runErc,
  generateNetlist,
  routeNet,
  fillZones,
  parseKicadPcb,
  builtinSymbols,
} from "./ecad.js";
export type {
  DrcViolationResult,
  ErcViolationResult,
  NetlistResult,
  NetlistNet,
  NetConnection,
  RouteResult,
  FilledZoneResult,
} from "./ecad.js";

// Physics simulation
export { PhysicsEnv, isPhysicsAvailable } from "./physics.js";
export type {
  PhysicsObservation,
  PhysicsStepResult,
  PhysicsEnvOptions,
  ActionType as PhysicsActionType,
} from "./physics.js";

/** Re-export Solid class for direct use */
export type { Solid, WasmAnnotationLayer } from "@vcad/kernel-wasm";

/** 2D projected edge with visibility info */
export interface ProjectedEdge {
  start: { x: number; y: number };
  end: { x: number; y: number };
  visibility: "Visible" | "Hidden";
  edge_type: "Sharp" | "Silhouette" | "Boundary";
  depth: number;
}

/** 2D bounding box */
export interface BoundingBox2D {
  min_x: number;
  min_y: number;
  max_x: number;
  max_y: number;
}

/** Result of projecting a 3D mesh to a 2D view */
export interface ProjectedView {
  edges: ProjectedEdge[];
  bounds: BoundingBox2D;
  view_direction: string;
}

/** Detail view parameters */
export interface DetailViewParams {
  center: { x: number; y: number };
  scale: number;
  width: number;
  height: number;
  label: string;
}

/** A magnified region view */
export interface DetailView {
  edges: ProjectedEdge[];
  bounds: BoundingBox2D;
  params: DetailViewParams;
}

/** Type for the initialized kernel module */
export interface KernelModule {
  Solid: typeof Solid;
  WasmAnnotationLayer: typeof WasmAnnotationLayer;
  projectMesh: (mesh: { positions: Float32Array; indices: Uint32Array }, viewDirection: string) => ProjectedView | null;
  importStepBuffer: (data: Uint8Array) => Array<{ positions: Float32Array; indices: Uint32Array }>;
  exportProjectedViewToDxf: (view_json: string) => Uint8Array;
  createDetailView: (
    parent_json: string,
    center_x: number,
    center_y: number,
    scale: number,
    width: number,
    height: number,
    label: string,
  ) => DetailView;
  /** Full document evaluator (Rust-side, handles all CsgOp variants). */
  evaluateDocument?: (docJson: string, skipClashDetection: boolean) => unknown;
  /** Evaluate loon source → JSON-serialized Document. */
  evalVcadSource?: (source: string) => string;
}

/** Rendered dimension types from the annotation layer */
export interface RenderedText {
  position: { x: number; y: number };
  text: string;
  height: number;
  rotation: number;
  alignment: string;
}

export interface RenderedArrow {
  tip: { x: number; y: number };
  direction: number;
  arrow_type: string;
  size: number;
}

export interface RenderedArc {
  center: { x: number; y: number };
  radius: number;
  start_angle: number;
  end_angle: number;
}

export interface RenderedDimension {
  lines: Array<[{ x: number; y: number }, { x: number; y: number }]>;
  arcs: RenderedArc[];
  arrows: RenderedArrow[];
  texts: RenderedText[];
  is_basic: boolean;
}

/** Maximum number of cached scenes to keep */
const SCENE_CACHE_MAX = 10;

/** CSG evaluation engine backed by vcad-kernel (WASM). */
export class Engine {
  /** Enable timing logs. Auto-detected from Vite/Node env, or set manually. */
  static DEV: boolean = (() => {
    try {
      // Vite injects import.meta.env at build time
      return !!(import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV;
    } catch {
      return typeof process !== "undefined" && process.env?.NODE_ENV !== "production";
    }
  })();

  /** Log full timing breakdown to console. */
  static logTiming(timing: import("./mesh.js").EvalTimingData, workerMs?: number): void {
    // Extract node entries — serde_wasm_bindgen may produce Map or Object
    const nodeEntries: [string, import("./mesh.js").NodeTimingData][] =
      timing.nodes instanceof Map
        ? [...timing.nodes.entries()]
        : Object.entries(timing.nodes);

    // Sort by eval_ms descending
    nodeEntries.sort((a, b) => b[1].eval_ms - a[1].eval_ms);

    // One-line summary: total + phases
    const summary = [
      `total:${timing.total_ms.toFixed(0)}ms`,
      timing.parse_ms != null ? `parse:${timing.parse_ms.toFixed(0)}ms` : null,
      `tess:${timing.tessellate_ms.toFixed(0)}ms`,
      timing.serialize_ms != null ? `ser:${timing.serialize_ms.toFixed(0)}ms` : null,
      timing.clash_ms > 0.5 ? `clash:${timing.clash_ms.toFixed(0)}ms` : null,
      timing.assembly_ms > 0.5 ? `asm:${timing.assembly_ms.toFixed(0)}ms` : null,
      workerMs != null ? `worker:${workerMs.toFixed(0)}ms` : null,
    ].filter(Boolean).join(" ");

    // Per-node breakdown: show all ops >1ms
    const ops = nodeEntries
      .filter(([, n]) => n.eval_ms > 1)
      .map(([id, n]) => `${n.op}#${id}:${n.eval_ms.toFixed(0)}ms${n.mesh_ms > 0.5 ? `(mesh:${n.mesh_ms.toFixed(0)})` : ""}`)
      .join(" > ");

    console.debug(`[ENGINE] ${summary}${ops ? `\n         ${ops}` : ""}`);
  }

  private kernel: KernelModule;

  /** Persistent cache for evaluated solids */
  readonly solidCache: SolidCache;

  /** Cache for tessellated meshes */
  readonly meshCache: MeshCache;

  /** Dependency graph for incremental evaluation */
  readonly dependencyGraph: DependencyGraph;

  /** Last evaluated document hash for change detection */
  private lastDocHash: string | null = null;

  /** Web Worker for off-main-thread evaluation */
  private worker: Worker | null = null;

  /** Resolves when the worker has finished WASM init */
  private workerReady: Promise<void> | null = null;

  /** Document-level scene cache (keyed by doc hash + options) */
  private sceneCache = new Map<string, EvaluatedScene>();

  private constructor(kernel: KernelModule, compiledWasmModule?: WebAssembly.Module) {
    this.kernel = kernel;
    this.solidCache = new SolidCache();
    this.meshCache = new MeshCache();
    this.dependencyGraph = new DependencyGraph();
    this.initWorker(compiledWasmModule);
  }

  /** Spin up the eval worker (browser only, best-effort). */
  private initWorker(compiledWasmModule?: WebAssembly.Module): void {
    // Workers only available in browser
    if (typeof Worker === "undefined") return;

    try {
      const worker = new Worker(
        new URL("./eval-worker.js", import.meta.url),
        { type: "module" },
      );

      this.workerReady = new Promise<void>((resolve, reject) => {
        const onMessage = (e: MessageEvent) => {
          if (e.data.type === "ready") {
            worker.removeEventListener("message", onMessage);
            resolve();
          } else if (e.data.type === "error" && e.data.id === null) {
            worker.removeEventListener("message", onMessage);
            console.warn("[ENGINE] Worker WASM init failed:", e.data.message);
            this.worker = null;
            this.workerReady = null;
            reject(new Error(e.data.message));
          }
        };
        worker.addEventListener("message", onMessage);
      });

      // Pass compiled WASM module to worker to avoid recompilation (~3s savings)
      worker.postMessage({ type: "init", module: compiledWasmModule });
      this.worker = worker;
    } catch (e) {
      console.warn("[ENGINE] Failed to create eval worker:", e);
    }
  }

  /**
   * Load the vcad-kernel WASM module and return a ready engine.
   *
   * The browser path accepts an optional pre-fetched WASM buffer or Response,
   * forwarded to wasm-bindgen's init. Callers that want real byte-level
   * download progress (see `packages/app/src/lib/bootstrap.ts`) fetch the
   * asset themselves and pass the buffer here.
   */
  static async init(opts?: {
    wasmInput?: BufferSource | Response;
  }): Promise<Engine> {
    const { getKernelWasm, primeKernelWasm } = await import("./wasm-singleton.js");
    if (opts?.wasmInput) primeKernelWasm(opts.wasmInput);
    const wasmModule = await getKernelWasm();

    // Get the compiled WebAssembly.Module to share with the worker.
    // This avoids a ~3s recompilation in the worker thread.
    const getCompiledModule = (wasmModule as Record<string, unknown>).getCompiledModule as (() => WebAssembly.Module | undefined) | undefined;
    const compiledWasmModule = getCompiledModule?.();

    return new Engine({
      Solid: wasmModule.Solid,
      WasmAnnotationLayer: wasmModule.WasmAnnotationLayer,
      projectMesh: wasmModule.projectMesh,
      importStepBuffer: wasmModule.importStepBuffer,
      exportProjectedViewToDxf: wasmModule.exportProjectedViewToDxf,
      createDetailView: wasmModule.createDetailView,
      evaluateDocument: (wasmModule as Record<string, unknown>).evaluateDocument as KernelModule["evaluateDocument"],
      evalVcadSource: (wasmModule as Record<string, unknown>).evalVcadSource as KernelModule["evalVcadSource"],
    }, compiledWasmModule);
  }

  /** Evaluate an IR document into triangle meshes (synchronous, main-thread). */
  evaluate(doc: Document, options: EvaluateOptions = {}): EvaluatedScene {
    // Rebuild dependency graph if document structure changed significantly
    const nodeCount = Object.keys(doc.nodes).length;
    const currentHash = `${nodeCount}:${doc.roots.length}`;
    if (this.lastDocHash !== currentHash) {
      this.dependencyGraph.build(doc);
      this.lastDocHash = currentHash;
    }

    // Check scene cache
    const cacheKey = this.sceneCacheKey(doc, options);
    const cached = this.sceneCache.get(cacheKey);
    if (cached) return cached;

    const scene = evaluateDocument(doc, this.kernel, options);
    this.cacheScene(cacheKey, scene);
    return scene;
  }

  /**
   * Evaluate a document off the main thread via Web Worker.
   * Falls back to synchronous evaluation if the worker is unavailable.
   */
  async evaluateAsync(doc: Document, options: EvaluateOptions = {}): Promise<EvaluatedScene> {
    // Rebuild dependency graph
    const nodeCount = Object.keys(doc.nodes).length;
    const currentHash = `${nodeCount}:${doc.roots.length}`;
    if (this.lastDocHash !== currentHash) {
      this.dependencyGraph.build(doc);
      this.lastDocHash = currentHash;
    }

    // Check scene cache first
    const cacheKey = this.sceneCacheKey(doc, options);
    const cached = this.sceneCache.get(cacheKey);
    if (cached) return cached;

    // Try worker path
    if (this.worker && this.workerReady) {
      try {
        await this.workerReady;
      } catch {
        // Worker init failed — fall through to sync
      }

      if (this.worker) {
        try {
          const scene = await this.evaluateInWorker(doc, options);
          this.cacheScene(cacheKey, scene);
          return scene;
        } catch (e) {
          console.warn("[ENGINE] Worker eval failed, falling back to sync:", e);
        }
      }
    }

    // Fallback: synchronous on main thread
    const scene = evaluateDocument(doc, this.kernel, options);
    this.cacheScene(cacheKey, scene);
    return scene;
  }

  /** Send an evaluate message to the worker and await the result. */
  private evaluateInWorker(doc: Document, options: EvaluateOptions): Promise<EvaluatedScene> {
    const worker = this.worker!;
    const id = Math.random().toString(36).slice(2);
    const skipClash = options.skipClashDetection ?? false;

    return new Promise<EvaluatedScene>((resolve, reject) => {
      const onMessage = (e: MessageEvent) => {
        if (e.data.id !== id) return;
        worker.removeEventListener("message", onMessage);

        if (e.data.type === "result") {
          const scene = e.data.scene as EvaluatedScene;
          // Log timing in dev mode
          if (scene.timing && Engine.DEV) {
            Engine.logTiming(scene.timing, e.data.workerTotalMs as number | undefined);
          }
          resolve(scene);
        } else if (e.data.type === "error") {
          reject(new Error(e.data.message));
        }
      };
      worker.addEventListener("message", onMessage);
      worker.postMessage({
        type: "evaluate",
        id,
        docJson: JSON.stringify(doc),
        skipClashDetection: skipClash,
      });
    });
  }

  /** Document content hash for scene caching. */
  private sceneCacheKey(doc: Document, options: EvaluateOptions): string {
    // Full content hash — JSON.stringify is fast for typical documents (<1ms)
    // and ensures cache correctness when node parameters change.
    const skipClash = options.skipClashDetection ?? false;
    return `${skipClash}:${JSON.stringify(doc)}`;
  }

  /** Insert a scene into the cache, evicting oldest if over limit. */
  private cacheScene(key: string, scene: EvaluatedScene): void {
    this.sceneCache.set(key, scene);
    if (this.sceneCache.size > SCENE_CACHE_MAX) {
      const oldest = this.sceneCache.keys().next().value;
      if (oldest !== undefined) this.sceneCache.delete(oldest);
    }
  }

  /**
   * Invalidate cached data for specific nodes.
   * Call this when you know which nodes have changed.
   */
  invalidateNodes(nodeIds: Set<NodeId>): void {
    // Get all affected nodes (including dependents)
    const affected = this.dependencyGraph.getAffectedNodes(nodeIds);
    this.solidCache.invalidate(affected);
  }

  /**
   * Clear all caches.
   */
  clearCaches(): void {
    this.solidCache.clear();
    this.meshCache.clear();
    this.sceneCache.clear();
  }

  /** Get the Solid class for direct use */
  get Solid(): typeof Solid {
    return this.kernel.Solid;
  }

  /** Get the WasmAnnotationLayer class for creating dimensions */
  get WasmAnnotationLayer(): typeof WasmAnnotationLayer {
    return this.kernel.WasmAnnotationLayer;
  }

  /**
   * Get the CpuRayTracer class for direct BRep rendering (if available).
   * Returns undefined if the cpu-raytrace feature is not enabled.
   */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  get CpuRayTracer(): any {
    return (this.kernel as any).CpuRayTracer;
  }

  /** Project a mesh to a 2D view */
  projectMesh(mesh: TriangleMesh, viewDirection: string): ProjectedView | null {
    return this.kernel.projectMesh(
      { positions: mesh.positions, indices: mesh.indices },
      viewDirection,
    );
  }

  /** Import solids from a STEP file buffer.
   *
   * Returns an array of triangle meshes, one for each body in the STEP file.
   */
  importStep(data: ArrayBuffer): TriangleMesh[] {
    const bytes = new Uint8Array(data);
    const meshes = this.kernel.importStepBuffer(bytes);
    return meshes.map((m) => ({
      positions: new Float32Array(m.positions),
      indices: new Uint32Array(m.indices),
    }));
  }

  /** Export a projected view to DXF format.
   *
   * Returns the DXF file content as a Uint8Array.
   */
  exportDrawingToDxf(view: ProjectedView): Uint8Array {
    const json = JSON.stringify(view);
    return this.kernel.exportProjectedViewToDxf(json);
  }

  /** Create a detail view (magnified region) from a projected view.
   *
   * @param view - The parent projected view
   * @param centerX - X coordinate of the region center
   * @param centerY - Y coordinate of the region center
   * @param scale - Magnification factor (e.g., 2.0 = 2x)
   * @param width - Width of the region to capture
   * @param height - Height of the region to capture
   * @param label - Label for the detail view (e.g., "A")
   */
  createDetailView(
    view: ProjectedView,
    centerX: number,
    centerY: number,
    scale: number,
    width: number,
    height: number,
    label: string,
  ): DetailView {
    const json = JSON.stringify(view);
    return this.kernel.createDetailView(json, centerX, centerY, scale, width, height, label);
  }

  /**
   * Evaluate loon source code and return a parsed Document.
   * Returns null if the kernel doesn't support loon evaluation.
   */
  evalVcadSource(source: string): Document | null {
    if (!this.kernel.evalVcadSource) return null;
    const json = this.kernel.evalVcadSource(source);
    return JSON.parse(json) as Document;
  }

  /** Evaluate a preview extrusion without adding to document */
  evaluateExtrudePreview(
    origin: Vec3,
    xDir: Vec3,
    yDir: Vec3,
    segments: SketchSegment2D[],
    direction: Vec3,
  ): TriangleMesh | null {
    if (segments.length === 0) return null;

    try {
      const profile = {
        origin: [origin.x, origin.y, origin.z],
        x_dir: [xDir.x, xDir.y, xDir.z],
        y_dir: [yDir.x, yDir.y, yDir.z],
        segments: segments.map((seg) => {
          if (seg.type === "Line") {
            return {
              type: "Line" as const,
              start: [seg.start.x, seg.start.y],
              end: [seg.end.x, seg.end.y],
            };
          } else {
            return {
              type: "Arc" as const,
              start: [seg.start.x, seg.start.y],
              end: [seg.end.x, seg.end.y],
              center: [seg.center.x, seg.center.y],
              ccw: seg.ccw,
            };
          }
        }),
      };

      const dirArray = new Float64Array([direction.x, direction.y, direction.z]);
      const solid = this.kernel.Solid.extrude(JSON.stringify(profile), dirArray);
      const meshData = solid.getMesh();

      return {
        positions: new Float32Array(meshData.positions),
        indices: new Uint32Array(meshData.indices),
      };
    } catch (e) {
      // Log the error instead of silently swallowing it. A panic inside
      // `Solid.extrude` poisons the wasm borrow tracking and the very next
      // `WasmDocumentEngine.add_feature` call will fail with "recursive
      // use of an object detected" — without this log, the root cause is
      // invisible.
      console.warn("[engine] evaluateExtrudePreview failed:", e);
      return null;
    }
  }

  /** Evaluate a preview revolve without adding to document */
  evaluateRevolvePreview(
    origin: Vec3,
    xDir: Vec3,
    yDir: Vec3,
    segments: SketchSegment2D[],
    axisOrigin: Vec3,
    axisDir: Vec3,
    angleDeg: number,
  ): TriangleMesh | null {
    if (segments.length === 0) return null;

    try {
      const profile = {
        origin: [origin.x, origin.y, origin.z],
        x_dir: [xDir.x, xDir.y, xDir.z],
        y_dir: [yDir.x, yDir.y, yDir.z],
        segments: segments.map((seg) => {
          if (seg.type === "Line") {
            return {
              type: "Line" as const,
              start: [seg.start.x, seg.start.y],
              end: [seg.end.x, seg.end.y],
            };
          } else {
            return {
              type: "Arc" as const,
              start: [seg.start.x, seg.start.y],
              end: [seg.end.x, seg.end.y],
              center: [seg.center.x, seg.center.y],
              ccw: seg.ccw,
            };
          }
        }),
      };

      const axisOriginArray = new Float64Array([axisOrigin.x, axisOrigin.y, axisOrigin.z]);
      const axisDirArray = new Float64Array([axisDir.x, axisDir.y, axisDir.z]);
      const solid = this.kernel.Solid.revolve(JSON.stringify(profile), axisOriginArray, axisDirArray, angleDeg);
      const meshData = solid.getMesh();

      return {
        positions: new Float32Array(meshData.positions),
        indices: new Uint32Array(meshData.indices),
      };
    } catch (e) {
      // See evaluateExtrudePreview — silent catches here poison wasm
      // borrows and break the next mutation.
      console.warn("[engine] evaluateRevolvePreview failed:", e);
      return null;
    }
  }
}
