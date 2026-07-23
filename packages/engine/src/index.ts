import type { Document, Vec3, SketchSegment2D, NodeId } from "@vcad/ir";
import {
  evaluateDocument,
  evaluateDocumentTS,
  convertSegment,
  type EvaluateOptions,
} from "./evaluate.js";
import type { EvaluatedScene, TriangleMesh } from "./mesh.js";
import type { Solid, WasmAnnotationLayer } from "@vcad/kernel-wasm";
import { SolidCache } from "./solid-cache.js";
import { MeshCache } from "./mesh-cache.js";
import { DependencyGraph } from "./dependency-graph.js";
import {
  buildSheetMetalChain,
  findSheetMetalChainRoot,
  checkSheetMetalManufacturability,
  costSheetMetalChain,
  sheetMetalSequence as runSheetMetalSequence,
  nestSheetMetalParts as runNestSheetMetalParts,
  getSheetMetalMaterials as readSheetMetalMaterials,
  getSheetMetalBendTable as readSheetMetalBendTable,
  getSheetMetalShopCatalog as readSheetMetalShopCatalog,
  foldedSheetMetalStep as buildFoldedSheetMetalStep,
} from "./sheet-metal.js";
import type {
  SheetMetalShopProfile,
  SheetMetalCheckResult,
  SheetMetalMaterial,
  SheetMetalBendTable,
  SheetMetalCostRates,
  SheetMetalCostResult,
  SheetMetalBendStep,
  SheetMetalPartFootprint,
  SheetMetalNestingParams,
  SheetMetalNestingResult,
  SheetMetalShopCatalog,
} from "./sheet-metal.js";

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
  resetKernelWasm,
  lastKernelTrapReason,
  kernelWasmGeneration,
} from "./wasm-singleton.js";

export {
  runDfm,
  estimateCost,
  getDefaultDfmPack,
} from "./dfm.js";
export type {
  DfmProcess,
  DfmSeverity,
  DfmFix,
  DfmIssue,
  DfmReport,
  DfmCostEstimate,
  RunDfmOptions,
  EstimateCostOptions,
} from "./dfm.js";

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
export {
  semanticDiff,
  semanticDiffFallback,
  threeWayMerge,
  mergeAvailable,
  type DocumentDiff,
  type EntityChange,
  type EntityKind as DiffEntityKind,
  type FieldChange,
  type MergeConflict,
  type MergeResolution,
  type MergeOutcome,
} from "./diff.js";
export { SolidCache, hashCsgOp } from "./solid-cache.js";
export { MeshCache } from "./mesh-cache.js";
export { DependencyGraph } from "./dependency-graph.js";
export type { EvaluateOptions } from "./evaluate.js";
// The authoritative TS-side Transform3D application (extrinsic X→Y→Z euler,
// matrix Rz·Ry·Rx) — exported so renderers can regression-test against it.
export { transformMesh } from "./evaluate.js";
export type { TransformInfo } from "./transform-walk.js";

// Animation sequencer — samples a document Timeline into per-frame state.
export {
  sampleSequence,
  poseDocument,
  sampleTrackValue,
} from "./sequence.js";
export type { CameraPose, SequenceFrame } from "./sequence.js";

// Sheet-metal — thin types that ride on EvaluatedPart.sheetMetal so the
// UI can render the flat pattern and bend list without re-querying WASM.
// All actual geometry lives in the Rust kernel.
export type {
  SheetMetalBendSummary,
  SheetMetalModelSummary,
  SheetMetalFlatCrease,
  SheetMetalFlatPattern,
  SheetMetalRendered,
  SheetMetalViolation,
  SheetMetalShopProfile,
  SheetMetalCheckResult,
  SheetMetalMaterial,
  SheetMetalBendTable,
  SheetMetalBendTableRow,
  SheetMetalCostRates,
  SheetMetalCostBreakdown,
  SheetMetalCostResult,
  SheetMetalBendStep,
  SheetMetalPartFootprint,
  SheetMetalNestingParams,
  SheetMetalNestingResult,
  SheetMetalPlacement,
  SheetMetalShopCatalog,
  SheetMetalShopCatalogMaterial,
  SheetMetalShopCatalogRow,
} from "./sheet-metal.js";
export {
  DEFAULT_SHOP_PROFILE,
  DEFAULT_COST_RATES,
  DEFAULT_NESTING_PARAMS,
} from "./sheet-metal.js";

// Parametric expressions
export {
  parse as parseExpression,
  evaluate as evaluateExpression,
  evalAst,
  evalExprSafe,
  freeVars as expressionFreeVars,
  resolveDocument,
  resolveParameters,
  parseBindingKey,
  ParseError as ExpressionParseError,
  EvalError as ExpressionEvalError,
} from "./expressions.js";
export type { Ast as ExpressionAst } from "./expressions.js";

// ECAD (Electronics)
export {
  isEcadAvailable,
  runDrc,
  runDrcInRegion,
  runPcbDfm,
  getPcbDfmPack,
  tryRunDrc,
  critiqueRoute,
  netContinuity,
  runErc,
  checkErc,
  generateNetlist,
  routeNet,
  routeNetShove,
  routeNetMaze,
  routeAll,
  routeDiffPair,
  matchTraceLengths,
  fillZones,
  exportFabFiles,
  tryExportFabFiles,
  parseKicadPcb,
  exportKicadPcb,
  exportKicadProject,
  exportKicadSch,
  builtinSymbols,
  footprintForName,
  resolveFootprint,
  computeRatsnest,
  componentMeshes,
  pcbPreviewMeshes,
  tryPcbPreviewMeshes,
  createCircuitSim,
  circuitFromSchematic,
  circuitDcOperatingPoint,
  circuitAcResponse,
  circuitTune,
  evaluateMotor,
  airgapFluxDensity,
  resolvePart,
  searchEcadParts,
  partsManifest,
  resolvePartDef,
  jellybeanManifest,
  findAlternatives,
  verifySubstitution,
  buildReceipt,
  verifyReceipt,
} from "./ecad.js";
export type {
  EcadProbe,
  DrcViolationResult,
  PcbFabProfile,
  PcbDfmSeverity,
  PcbDfmLocation,
  PcbDfmRuleResult,
  PcbDfmReport,
  ErcViolationResult,
  ErcOutcome,
  NetlistResult,
  NetlistNet,
  NetConnection,
  RouteResult,
  RouteAllResult,
  RoutedTrace,
  RoutedVia,
  UnroutedDiagnostic,
  LengthMatchOptions,
  LengthMatchResult,
  NetLengthReport,
  FilledZoneResult,
  FabFile,
  RatsnestLine,
  ComponentMesh,
  PcbPreviewMesh,
  PcbPreviewEntity,
  CircuitObservation,
  CircuitSimHandle,
  CircuitSpecDevice,
  CircuitBlocker,
  CircuitMapOptions,
  CircuitMapResult,
  CircuitDcResult,
  CircuitAcResult,
  CircuitTuneResult,
  FootprintTemplate,
  FootprintResolution,
  MotorSpecInput,
  MotorPerformanceResult,
  AirGapSpecInput,
  SpecValue,
  ComponentClass,
  ElecXref,
  ResolvedPart,
  ResolvedPartDef,
  PartDefPin,
  FootprintCompat,
  Alternative,
  Substitution,
  NetContinuity,
  NetIsland,
} from "./ecad.js";

// Parts library
export {
  loadPartsManifest,
  clearPartsManifestCache,
  defaultParamsFor,
  searchParts,
  buildPartDocument,
} from "./parts.js";
export type { PartManifestEntry, PartParam, PartXref } from "./parts.js";

// Analyze mode (#592): off-main-thread solver studies
export { AnalyzeClient, getAnalyzeClient } from "./analyze.js";
export type {
  FeaAnalysis,
  FeaStudyResult,
  FeaSolutionSummary,
  ToleranceAnalysis,
  ToleranceDistribution,
} from "./analyze.js";

// Physics simulation
export { PhysicsEnv, isPhysicsAvailable } from "./physics.js";
export type {
  PhysicsObservation,
  PhysicsStepResult,
  PhysicsEnvOptions,
  ActionType as PhysicsActionType,
} from "./physics.js";

// Atomic / molecular domain: structure I/O, MD, minimization, receipts.
export {
  MdEnv,
  isAtomsAvailable,
  parseXyz,
  writeXyz,
  inspectMolecule,
  minimizeEnergy,
  homogenizeMaterial,
  buildReceipt as buildMoleculeReceipt,
} from "./atoms.js";
export type {
  MoleculeReport,
  MdObservation,
  MinimizeResult as AtomsMinimizeResult,
  MdConfig,
  MaterialCard,
} from "./atoms.js";

// Cross-domain PCB ↔ enclosure verification (pure; no kernel dependency)
export {
  checkEnclosureFit,
  deriveBoardFromCavity,
  mountingHolesFromPcb,
  connectorsFromPcb,
  componentExtentsFromMeshes,
  outlineAabb,
  toWorld,
} from "./enclosure-fit.js";
export type {
  EnclosureCavity,
  EnclosureFeatures,
  EnclosureFitInput,
  EnclosureFitReport,
  EnclosureFitCheck,
  CheckStatus as EnclosureCheckStatus,
  BoardPlacement,
  MountingHole,
  ConnectorRef,
  ComponentExtent,
  Standoff,
  WallOpening,
  WallEdge,
  DeriveBoardOptions,
} from "./enclosure-fit.js";
export { extractEnclosureFeatures } from "./enclosure-mesh.js";

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

/** A 2D section cut curve (from sectioning a mesh with a plane) */
export interface SectionCurve {
  points: Array<{ x: number; y: number }>;
  is_closed: boolean;
}

/** Result of sectioning a mesh: cut curves, hatch lines, bounds */
export interface SectionView {
  curves: SectionCurve[];
  hatch_lines: Array<[{ x: number; y: number }, { x: number; y: number }]>;
  bounds: BoundingBox2D;
}

/** A cutting plane for section views */
export interface SectionPlane {
  origin: [number, number, number];
  normal: [number, number, number];
  up: [number, number, number];
}

/** One step (jog span) of an offset section */
export interface OffsetSectionStep {
  u_start: number;
  u_end: number;
  offset: number;
}

/** An offset (stepped) section plane: base plane + jogs */
export interface OffsetSectionPlane {
  base: SectionPlane;
  steps: OffsetSectionStep[];
}

/** Title block field values for a drawing sheet (kernel snake_case shape) */
export interface TitleBlockFields {
  part_name: string;
  material: string;
  finish: string;
  scale: string;
  drawn_by: string;
  date: string;
  revision: string;
  units: string;
  tolerance_note: string;
}

/** One row of a drawing BOM table */
export interface BomRow {
  item: number;
  name: string;
  qty: number;
  material: string;
}

/** A rendered drawing block (title block / BOM table) plus its footprint */
export interface RenderedBlock {
  rendered: RenderedDimension;
  width: number;
  height: number;
}

/** Spec for composing a drawing sheet for PDF export */
export interface DrawingSheetSpec {
  size?: "a4" | "a3" | "letter" | { custom: { width: number; height: number } };
  views?: Array<{ view: ProjectedView; center: [number, number]; scale: number; label?: string }>;
  sections?: Array<{ view: SectionView; center: [number, number]; scale: number; label?: string }>;
  annotations?: RenderedDimension[];
  title_block?: TitleBlockFields;
  bom?: BomRow[];
}

/** Type for the initialized kernel module */
export interface KernelModule {
  Solid: typeof Solid;
  WasmAnnotationLayer: typeof WasmAnnotationLayer;
  projectMesh: (mesh: { positions: Float32Array; indices: Uint32Array }, viewDirection: string) => ProjectedView | null;
  importStepBuffer: (data: Uint8Array) => Array<{ positions: Float32Array; indices: Uint32Array }>;
  /**
   * Import a URDF (Unified Robot Description Format) file. Returns a
   * JSON-encoded {@link Document} that the caller deserialises with
   * `Document.fromJson` (or hands directly to the document store).
   *
   * `<mesh>` references in the URDF are resolved best-effort: the
   * browser cannot read the user's filesystem, so any mesh path that
   * isn't already absolute on a virtual FS falls back to a 1cm
   * placeholder cube. Joint topology and `<inertial>` mass / inertia
   * still flow through correctly, so simulation behaves like the real
   * robot to first order.
   */
  importUrdfBuffer: (data: Uint8Array) => string;
  exportProjectedViewToDxf: (view_json: string) => Uint8Array;
  /** Offset (stepped) section of a mesh; null on parse failure. */
  offsetSectionMesh?: (
    mesh: { positions: Float32Array; indices: Uint32Array },
    plane_json: string,
    hatch_json?: string,
  ) => SectionView | null;
  /** Render a title block at origin (0,0). */
  renderTitleBlock?: (fields_json: string) => RenderedBlock | null;
  /** Render a BOM table at origin (0,0). */
  renderBomTable?: (rows_json: string) => RenderedBlock | null;
  /** Compose a drawing sheet and export it as PDF bytes. */
  drawingSheetToPdf?: (spec_json: string) => Uint8Array;
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
  /** d(mass-property + bbox QoIs)/dθ for a named document parameter. */
  documentParameterGradient?: (
    docJson: string,
    parameter: string,
    density: number,
    probeStep: number,
  ) => unknown;
  /** JSON-serialized parts manifest for the stdlib. */
  getPartsManifest?: () => string;
  /** Build a stdlib part's sub-document given path and params JSON. */
  buildPart?: (path: string, paramsJson: string) => string;
  /** Evaluate a sheet-metal op chain → mesh + flat pattern + summary JSON. */
  evaluateSheetMetalChain?: (chainJson: string) => string;
  /** Run sheet-metal manufacturability vs. a shop profile → JSON. */
  checkSheetMetal?: (chainJson: string, shopJson: string) => string;
  /** Estimate sheet-metal cost for a chain → JSON. */
  costSheetMetal?: (chainJson: string, ratesJson: string, quantity: number) => string;
  /** Compute a bend sequence for a chain → JSON. */
  sheetMetalSequence?: (chainJson: string) => string;
  /** Nest multiple part footprints on stock sheets → JSON. */
  nestSheetMetalParts?: (partsJson: string, paramsJson: string) => string;
  /** Built-in materials registry → JSON array. */
  getSheetMetalMaterials?: () => string;
  /** Built-in bend table → JSON `{id, rows}`. */
  getSheetMetalBendTable?: () => string;
  /** Built-in shop catalog (e.g. `"sendcutsend"`) → JSON or `{error}`. */
  getSheetMetalShopCatalog?: (shopId: string) => string;
  /** Folded sheet-metal solid as STEP AP214 → JSON `{step, error}`. */
  sheetMetalFoldedStep?: (chainJson: string) => string;
  /** Mesh-to-mesh clearance over raw evaluated-mesh buffers. */
  mesh_clearance?: (
    positionsA: Float32Array,
    indicesA: Uint32Array,
    positionsB: Float32Array,
    indicesB: Uint32Array,
  ) => ClearanceResult;
  /** SIMP topology optimization over a box design domain. */
  topologyOptimizeBox?: (
    specJson: string,
    minX: number,
    minY: number,
    minZ: number,
    maxX: number,
    maxY: number,
    maxZ: number,
  ) => unknown;
  /** SIMP topology optimization inside a closed evaluated mesh. */
  topologyOptimizeMesh?: (
    specJson: string,
    positions: Float32Array,
    indices: Uint32Array,
  ) => unknown;
  /** Charged-particle optics simulation (DeviceSpec/params/options JSON). */
  particleSimulate?: (
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ) => unknown;
  /** Charged-particle yield optimization over named spec parameters. */
  particleOptimize?: (
    specJson: string,
    paramsJson: string,
    optimizeJson: string,
  ) => unknown;
  /** Tolerance stackup analysis (StackupSpec/params/options JSON). */
  toleranceAnalyze?: (
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ) => unknown;
  /** Steady heat-conduction solve (ThermalSpec/params/options JSON). */
  thermalSolve?: (
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ) => unknown;
  /** Transient heat-conduction solve (adds a TransientSpec schedule). */
  thermalSolveTransient?: (
    specJson: string,
    transientJson: string,
    paramsJson: string,
    optionsJson: string,
  ) => unknown;
  /** Semantic entity-level diff of two `.vcad` documents (JSON strings). */
  documentDiff?: (oldJson: string, newJson: string) => unknown;
  /** Apply a `DocumentDiff` to a document, returning the patched document. */
  documentDiffApply?: (oldJson: string, diffJson: string) => unknown;
  /** Fail-closed three-way merge with optional conflict resolutions. */
  documentMerge?: (
    baseJson: string,
    oursJson: string,
    theirsJson: string,
    resolutionsJson?: string | null,
  ) => unknown;
  /** Human-readable rendering of a `DocumentDiff` JSON string. */
  documentDiffHuman?: (diffJson: string) => string;
  /** Circuit DC operating point (`{devices:[...]}` spec JSON). */
  circuitDcOperatingPoint?: (specJson: string) => unknown;
  /** Circuit small-signal AC sweep at the given angular frequencies. */
  circuitAcResponse?: (
    specJson: string,
    sourceId: number,
    omegas: Float64Array | number[],
  ) => unknown;
  /** Circuit adjoint sensitivities (`{"dc":true}` or `{"ac":{...}}`). */
  circuitSensitivities?: (
    specJson: string,
    outNode: number,
    analysisJson: string,
  ) => unknown;
  /** Batched circuit transient run (trapezoidal). */
  circuitTransient?: (
    specJson: string,
    steps: number,
    sampleEvery: number,
  ) => unknown;
  /** Adjoint gradient-descent circuit tuning (TuneSpec JSON). */
  circuitTune?: (specJson: string, tuneJson: string) => unknown;
  /** Static structural FEA with convergence gating (FeaSpec/options JSON + mesh). */
  feaAnalyzeMesh?: (
    specJson: string,
    optionsJson: string,
    positions: Float32Array,
    indices: Uint32Array,
  ) => unknown;
  /** EM field simulation (problem-tagged spec/params/options JSON). */
  emSimulate?: (
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ) => unknown;
  /** Thin-wire MoM antenna analysis (AntennaSpec/params/options JSON). */
  antennaAnalyze?: (
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ) => unknown;
  /** Forward 2D FDTD photonics run (device spec/options JSON). */
  photonicsSimulate?: (specJson: string, optionsJson: string) => unknown;
  /** Monte Carlo neutron shielding run (ShieldSpec/params JSON). */
  neutronicsSimulate?: (specJson: string, paramsJson: string) => unknown;
  /** Static structural analysis of a box solid. */
  analyzeStaticsBox?: (
    specJson: string,
    minX: number,
    minY: number,
    minZ: number,
    maxX: number,
    maxY: number,
    maxZ: number,
  ) => unknown;
  /** Static structural analysis inside a closed evaluated mesh. */
  analyzeStaticsMesh?: (
    specJson: string,
    positions: Float32Array,
    indices: Uint32Array,
  ) => unknown;
}

/**
 * Static analysis parameters (mirrors `vcad_kernel_topopt::AnalysisSpec`;
 * unset fields take the kernel defaults). Resolution is the fidelity dial:
 * 32 is the fast predict tier, 64–96 the verify tier.
 */
export interface StaticAnalysisSpec {
  loads: TopoOptLoad[];
  supports: TopoOptSupport[];
  /** Voxels along the longest domain axis. Default 32. */
  resolution?: number;
  /** Young's modulus in MPa (N/mm²). Default 69 000 (6061 aluminum). */
  youngs_modulus_mpa?: number;
  /** Poisson's ratio. Default 0.33. */
  poisson?: number;
}

/** Result of a static analysis solve (mirrors `WasmStaticAnalysis`). */
export interface StaticAnalysisResult {
  /** Compliance `fᵀu` in N·mm (lower = stiffer under these loads). */
  compliance: number;
  /** Maximum nodal displacement magnitude in mm. */
  maxDisplacementMm: number;
  /** World position of the most-displaced node, mm. */
  maxDisplacementAt: [number, number, number];
  /** Max element-centroid von Mises stress in MPa (voxel estimate). */
  maxVonMisesMpa: number;
  /** World position of the most-stressed element centroid, mm. */
  maxStressAt: [number, number, number];
  /** Voxel grid dimensions `[nx, ny, nz]`. */
  grid: [number, number, number];
  /** Voxel edge length in mm. */
  voxelSizeMm: number;
  /** Relative residual the PCG solve reached. */
  relativeResidual: number;
  /** Whether the solve converged. */
  converged: boolean;
}

/** Axis-aligned box region (mm) selecting grid nodes for loads/supports. */
export interface TopoOptRegion {
  /** Minimum corner `[x, y, z]` in mm. */
  min: [number, number, number];
  /** Maximum corner `[x, y, z]` in mm. */
  max: [number, number, number];
}

/** A load for topology optimization: total force spread over a region. */
export interface TopoOptLoad {
  region: TopoOptRegion;
  /** Total force vector `[fx, fy, fz]` (any consistent unit). */
  force: [number, number, number];
}

/** A support (fixed boundary) for topology optimization. */
export interface TopoOptSupport {
  region: TopoOptRegion;
  /** Which translational directions are fixed; defaults to fully fixed. */
  fix?: [boolean, boolean, boolean];
}

/**
 * Topology optimization parameters (mirrors `vcad_kernel_topopt::TopoOptSpec`;
 * unset fields take the kernel defaults).
 */
export interface TopoOptSpec {
  loads: TopoOptLoad[];
  supports: TopoOptSupport[];
  /** Material fraction of the domain to keep, in (0, 1). Default 0.3. */
  volume_fraction?: number;
  /** Voxels along the longest domain axis (8–128). Default 48. */
  resolution?: number;
  /** SIMP penalization exponent. Default 3. */
  penalty?: number;
  /** Sensitivity filter radius in voxels. Default 1.5. */
  filter_radius?: number;
  /** Max optimization iterations. Default 40. */
  max_iterations?: number;
  /** Convergence tolerance on density change. Default 0.01. */
  tolerance?: number;
  /** Poisson's ratio. Default 0.3. */
  poisson?: number;
  /** Taubin smoothing passes on the extracted surface. Default 5. */
  smooth_iterations?: number;
}

/** Result of a topology optimization run (mirrors `WasmTopoOptResult`). */
export interface TopoOptResult {
  /** Optimized structure as a watertight surface mesh (mm, Z-up). */
  mesh: {
    positions: Float32Array | number[];
    indices: Uint32Array | number[];
    normals?: Float32Array | number[];
  };
  /** Compliance after each SIMP iteration (decreasing = stiffer). */
  complianceHistory: number[];
  /** SIMP iterations actually run. */
  iterations: number;
  /** Whether the density change converged below the tolerance. */
  converged: boolean;
  /** Material fraction of the design domain actually used. */
  volumeFraction: number;
  /** Voxel grid dimensions `[nx, ny, nz]`. */
  grid: [number, number, number];
  /** Voxel edge length in mm. */
  voxelSize: number;
}

/**
 * Mesh-to-mesh clearance: minimum separation distance in mm, or the negated
 * deepest penetration when the meshes intersect (mirrors `WasmClearance`).
 */
export interface ClearanceResult {
  /** Signed distance in mm: `>= 0` separation, `< 0` penetration depth. */
  distance: number;
  /** True when the meshes intersect (crossing surfaces or containment). */
  intersecting: boolean;
  /** Point on the first mesh realizing the reported distance. */
  pointA: [number, number, number];
  /** Point on the second mesh realizing the reported distance. */
  pointB: [number, number, number];
}

/**
 * Per-part gradient of the mass-property + bounding-box QoIs with respect to
 * a single named document parameter (`d QoI / dθ`), from the differentiable
 * seam. Volume/mass/centroid derivatives are exact analytic seam evaluations;
 * `dBboxExtents` is a central finite difference (a bbox extent is a
 * non-smooth max over vertices).
 */
export interface PartParameterGradient {
  /** Index of the part in the evaluated scene's solid-part order. */
  partIndex: number;
  /** Signed volume at θ (mm³). */
  volume: number;
  /** dVolume/dθ (analytic). */
  dVolume: number;
  /** Mass at θ (`density · volume`). */
  mass: number;
  /** dMass/dθ (analytic). */
  dMass: number;
  /** Centroid `[x, y, z]` at θ. */
  centroid: [number, number, number];
  /** dCentroid/dθ `[x, y, z]` (analytic). */
  dCentroid: [number, number, number];
  /** AABB extents `[x, y, z]` at θ. */
  bboxExtents: [number, number, number];
  /** dBboxExtents/dθ `[x, y, z]` (central finite difference). */
  dBboxExtents: [number, number, number];
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

  /**
   * Resolves once the eval worker's WASM init is done (or the worker path is
   * unavailable). Bootstrap awaits this before transitioning to the
   * "evaluating" phase so the splash doesn't lie about what's happening.
   */
  whenWorkerReady(): Promise<void> {
    return this.workerReady ?? Promise.resolve();
  }

  /** Document-level scene cache (keyed by doc hash + options) */
  private sceneCache = new Map<string, EvaluatedScene>();

  /**
   * Separate cache for scenes that include BRep `solid` handles.
   * Worker-eval'd scenes drop solids (handles can't cross threads), so we
   * can't share a cache with `sceneCache` — a hit there might be
   * solid-less even though the caller asked for solids.
   */
  private solidSceneCache = new Map<string, EvaluatedScene>();

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
      importUrdfBuffer: (wasmModule as Record<string, unknown>).importUrdfBuffer as KernelModule["importUrdfBuffer"],
      exportProjectedViewToDxf: wasmModule.exportProjectedViewToDxf,
      offsetSectionMesh: (wasmModule as Record<string, unknown>).offsetSectionMesh as KernelModule["offsetSectionMesh"],
      renderTitleBlock: (wasmModule as Record<string, unknown>).renderTitleBlock as KernelModule["renderTitleBlock"],
      renderBomTable: (wasmModule as Record<string, unknown>).renderBomTable as KernelModule["renderBomTable"],
      drawingSheetToPdf: (wasmModule as Record<string, unknown>).drawingSheetToPdf as KernelModule["drawingSheetToPdf"],
      createDetailView: wasmModule.createDetailView,
      evaluateDocument: (wasmModule as Record<string, unknown>).evaluateDocument as KernelModule["evaluateDocument"],
      evalVcadSource: (wasmModule as Record<string, unknown>).evalVcadSource as KernelModule["evalVcadSource"],
      documentParameterGradient: (wasmModule as Record<string, unknown>).documentParameterGradient as KernelModule["documentParameterGradient"],
      getPartsManifest: (wasmModule as Record<string, unknown>).getPartsManifest as KernelModule["getPartsManifest"],
      buildPart: (wasmModule as Record<string, unknown>).buildPart as KernelModule["buildPart"],
      evaluateSheetMetalChain: (wasmModule as Record<string, unknown>).evaluateSheetMetalChain as KernelModule["evaluateSheetMetalChain"],
      checkSheetMetal: (wasmModule as Record<string, unknown>).checkSheetMetal as KernelModule["checkSheetMetal"],
      costSheetMetal: (wasmModule as Record<string, unknown>).costSheetMetal as KernelModule["costSheetMetal"],
      sheetMetalSequence: (wasmModule as Record<string, unknown>).sheetMetalSequence as KernelModule["sheetMetalSequence"],
      nestSheetMetalParts: (wasmModule as Record<string, unknown>).nestSheetMetalParts as KernelModule["nestSheetMetalParts"],
      getSheetMetalMaterials: (wasmModule as Record<string, unknown>).getSheetMetalMaterials as KernelModule["getSheetMetalMaterials"],
      getSheetMetalBendTable: (wasmModule as Record<string, unknown>).getSheetMetalBendTable as KernelModule["getSheetMetalBendTable"],
      getSheetMetalShopCatalog: (wasmModule as Record<string, unknown>).getSheetMetalShopCatalog as KernelModule["getSheetMetalShopCatalog"],
      sheetMetalFoldedStep: (wasmModule as Record<string, unknown>).sheetMetalFoldedStep as KernelModule["sheetMetalFoldedStep"],
      mesh_clearance: (wasmModule as Record<string, unknown>).mesh_clearance as KernelModule["mesh_clearance"],
      topologyOptimizeBox: (wasmModule as Record<string, unknown>).topologyOptimizeBox as KernelModule["topologyOptimizeBox"],
      topologyOptimizeMesh: (wasmModule as Record<string, unknown>).topologyOptimizeMesh as KernelModule["topologyOptimizeMesh"],
      particleSimulate: (wasmModule as Record<string, unknown>).particleSimulate as KernelModule["particleSimulate"],
      particleOptimize: (wasmModule as Record<string, unknown>).particleOptimize as KernelModule["particleOptimize"],
      toleranceAnalyze: (wasmModule as Record<string, unknown>).toleranceAnalyze as KernelModule["toleranceAnalyze"],
      thermalSolve: (wasmModule as Record<string, unknown>).thermalSolve as KernelModule["thermalSolve"],
      thermalSolveTransient: (wasmModule as Record<string, unknown>).thermalSolveTransient as KernelModule["thermalSolveTransient"],
      documentDiff: (wasmModule as Record<string, unknown>).documentDiff as KernelModule["documentDiff"],
      documentDiffApply: (wasmModule as Record<string, unknown>).documentDiffApply as KernelModule["documentDiffApply"],
      documentMerge: (wasmModule as Record<string, unknown>).documentMerge as KernelModule["documentMerge"],
      documentDiffHuman: (wasmModule as Record<string, unknown>).documentDiffHuman as KernelModule["documentDiffHuman"],
      circuitDcOperatingPoint: (wasmModule as Record<string, unknown>).circuitDcOperatingPoint as KernelModule["circuitDcOperatingPoint"],
      circuitAcResponse: (wasmModule as Record<string, unknown>).circuitAcResponse as KernelModule["circuitAcResponse"],
      circuitSensitivities: (wasmModule as Record<string, unknown>).circuitSensitivities as KernelModule["circuitSensitivities"],
      circuitTransient: (wasmModule as Record<string, unknown>).circuitTransient as KernelModule["circuitTransient"],
      circuitTune: (wasmModule as Record<string, unknown>).circuitTune as KernelModule["circuitTune"],
      feaAnalyzeMesh: (wasmModule as Record<string, unknown>).feaAnalyzeMesh as KernelModule["feaAnalyzeMesh"],
      emSimulate: (wasmModule as Record<string, unknown>).emSimulate as KernelModule["emSimulate"],
      antennaAnalyze: (wasmModule as Record<string, unknown>).antennaAnalyze as KernelModule["antennaAnalyze"],
      photonicsSimulate: (wasmModule as Record<string, unknown>).photonicsSimulate as KernelModule["photonicsSimulate"],
      neutronicsSimulate: (wasmModule as Record<string, unknown>).neutronicsSimulate as KernelModule["neutronicsSimulate"],
      analyzeStaticsBox: (wasmModule as Record<string, unknown>).analyzeStaticsBox as KernelModule["analyzeStaticsBox"],
      analyzeStaticsMesh: (wasmModule as Record<string, unknown>).analyzeStaticsMesh as KernelModule["analyzeStaticsMesh"],
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
   * Differentiate a document's mass-property + bounding-box QoIs with respect
   * to a single named parameter (`d QoI / dθ`) via the differentiable seam.
   * Returns one entry per solid part. Throws if `parameter` is not declared in
   * `doc.parameters`, or if the parameter crosses a topology/part-count
   * boundary at its current value.
   */
  parameterGradient(
    doc: Document,
    parameter: string,
    options: { density?: number; probeStep?: number } = {},
  ): PartParameterGradient[] {
    const fn = this.kernel.documentParameterGradient;
    if (typeof fn !== "function") {
      throw new Error(
        "kernel WASM is missing documentParameterGradient — rebuild @vcad/kernel-wasm",
      );
    }
    const density = options.density ?? 1.0;
    const probeStep = options.probeStep ?? 0;
    return fn(
      JSON.stringify(doc),
      parameter,
      density,
      probeStep,
    ) as PartParameterGradient[];
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

  /**
   * Evaluate a document on the main thread and return a scene with BRep
   * `solid` handles populated on each part. Callers that need handles
   * (ray tracing, STEP export) must use this entry point — both the worker
   * eval and the WASM main-thread evaluator drop solids, so this routes
   * through the TS evaluator which keeps them.
   *
   * Uses a separate cache from `evaluate()` so a solid-less scene cached
   * for the same doc doesn't shadow the result.
   */
  evaluateWithSolids(doc: Document, options: EvaluateOptions = {}): EvaluatedScene {
    const nodeCount = Object.keys(doc.nodes).length;
    const currentHash = `${nodeCount}:${doc.roots.length}`;
    if (this.lastDocHash !== currentHash) {
      this.dependencyGraph.build(doc);
      this.lastDocHash = currentHash;
    }

    const cacheKey = this.sceneCacheKey(doc, options);
    const cached = this.solidSceneCache.get(cacheKey);
    if (cached) return cached;

    const scene = evaluateDocumentTS(doc, this.kernel, options);
    this.solidSceneCache.set(cacheKey, scene);
    if (this.solidSceneCache.size > SCENE_CACHE_MAX) {
      const oldest = this.solidSceneCache.keys().next().value;
      if (oldest !== undefined) this.solidSceneCache.delete(oldest);
    }
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
    this.solidSceneCache.clear();
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

  /**
   * Minimum signed distance between two evaluated meshes in mm: `>= 0` is
   * the separation, `< 0` the deepest penetration when they intersect.
   * Meshes are measured as placed (positions are used verbatim), so callers
   * can compare any two parts of an evaluated scene.
   */
  meshClearance(a: TriangleMesh, b: TriangleMesh): ClearanceResult {
    if (typeof this.kernel.mesh_clearance !== "function") {
      throw new Error(
        "mesh_clearance is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return this.kernel.mesh_clearance(a.positions, a.indices, b.positions, b.indices);
  }

  /**
   * SIMP topology optimization over a box design domain: find the stiffest
   * material layout inside `[min, max]` (mm) using only
   * `spec.volume_fraction` of the volume, under the spec's loads and
   * supports. Returns the optimized structure as a watertight mesh plus run
   * diagnostics.
   */
  /**
   * Charged-particle optics simulation: solve an axisymmetric electrode
   * device, trace a deuteron ensemble, and return figures of merit plus
   * predicted receipt claims. Inputs are JSON strings (DeviceSpec, named
   * parameter bindings, options); see `vcad-kernel-particle`.
   */
  particleSimulate(
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ): unknown {
    const fn = this.kernel.particleSimulate;
    if (typeof fn !== "function") {
      throw new Error(
        "particleSimulate is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson, optionsJson);
  }

  /**
   * Multi-start gradient ascent over named device-spec parameters against
   * predicted D-D yield per ion. Inputs are JSON strings; see
   * `vcad-kernel-particle::optimize`.
   */
  particleOptimize(
    specJson: string,
    paramsJson: string,
    optimizeJson: string,
  ): unknown {
    const fn = this.kernel.particleOptimize;
    if (typeof fn !== "function") {
      throw new Error(
        "particleOptimize is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson, optimizeJson);
  }

  /**
   * Tolerance stackup analysis: worst-case, RSS, seeded Monte Carlo, and
   * exact sensitivities over a linear assembly chain, plus predicted
   * receipt claims. Inputs are JSON strings (StackupSpec, named parameter
   * bindings, options); see `vcad-kernel-tolerance`.
   */
  toleranceAnalyze(
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ): unknown {
    const fn = this.kernel.toleranceAnalyze;
    if (typeof fn !== "function") {
      throw new Error(
        "toleranceAnalyze is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson, optionsJson);
  }

  /**
   * Steady heat-conduction solve on a voxel grid: T_max, per-source theta
   * (junction-to-ambient), energy balance, and predicted receipt claims.
   * Inputs are JSON strings (ThermalSpec, named parameter bindings,
   * options); see `vcad-kernel-thermal`.
   */
  thermalSolve(
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ): unknown {
    const fn = this.kernel.thermalSolve;
    if (typeof fn !== "function") {
      throw new Error(
        "thermalSolve is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson, optionsJson);
  }

  /**
   * Transient heat-conduction solve: backward-Euler stepping over a
   * piecewise-constant drive schedule (TransientSpec JSON), returning
   * T_max/per-source time series, the final-state summary, and the
   * integrated energy audit with predicted receipt claims.
   */
  thermalSolveTransient(
    specJson: string,
    transientJson: string,
    paramsJson: string,
    optionsJson: string,
  ): unknown {
    const fn = this.kernel.thermalSolveTransient;
    if (typeof fn !== "function") {
      throw new Error(
        "thermalSolveTransient is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, transientJson, paramsJson, optionsJson);
  }

  private circuitFn<K extends keyof KernelModule>(name: K): NonNullable<KernelModule[K]> {
    const fn = this.kernel[name];
    if (typeof fn !== "function") {
      throw new Error(
        `${String(name)} is not exported by this kernel WASM build — rebuild packages/kernel-wasm`,
      );
    }
    return fn as NonNullable<KernelModule[K]>;
  }

  /**
   * Circuit DC operating point: node voltages, device currents, the Tellegen
   * power-balance residual, and predicted `vcad.spice-claims/1` claims.
   * `specJson` is the same `{devices:[{kind,p,n,value}]}` shape `CircuitSim`
   * takes; see `vcad-ecad-sim::circuit`.
   */
  circuitDcOperatingPoint(specJson: string): unknown {
    return this.circuitFn("circuitDcOperatingPoint")(specJson);
  }

  /** Small-signal AC sweep: per-omega complex node voltages (re/im arrays). */
  circuitAcResponse(
    specJson: string,
    sourceId: number,
    omegas: number[],
  ): unknown {
    return this.circuitFn("circuitAcResponse")(
      specJson,
      sourceId,
      new Float64Array(omegas),
    );
  }

  /** Adjoint sensitivities of the voltage at `outNode` to every device primary. */
  circuitSensitivities(
    specJson: string,
    outNode: number,
    analysisJson: string,
  ): unknown {
    return this.circuitFn("circuitSensitivities")(specJson, outNode, analysisJson);
  }

  /** Batched circuit transient run (trapezoidal integrator). */
  circuitTransient(
    specJson: string,
    steps: number,
    sampleEvery: number,
  ): unknown {
    return this.circuitFn("circuitTransient")(specJson, steps, sampleEvery);
  }

  /** Adjoint gradient-descent tuning toward a filter or DC target. */
  circuitTune(specJson: string, tuneJson: string): unknown {
    return this.circuitFn("circuitTune")(specJson, tuneJson);
  }

  /**
   * Static structural FEA on a closed triangle mesh: lattice tet fill at
   * two or more refinement levels, linear-elastic solve, and fail-closed
   * convergence gating (Unverifiable when QoIs disagree across levels).
   * Returns the study, the `vcad.fea-claims/1` set (converged only), and
   * unified receipt claims; see `vcad-kernel-fea`.
   */
  feaAnalyzeMesh(
    specJson: string,
    optionsJson: string,
    positions: Float32Array,
    indices: Uint32Array,
  ): unknown {
    const fn = this.kernel.feaAnalyzeMesh;
    if (typeof fn !== "function") {
      throw new Error(
        "feaAnalyzeMesh is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, optionsJson, positions, indices);
  }

  /**
   * EM field simulation (2D/axisymmetric finite-volume magnetostatics or
   * electrostatics): inductance, force, torque, capacitance extraction and
   * predicted receipt claims. The spec JSON carries a `problem` tag; see
   * `vcad-kernel-em`.
   */
  emSimulate(
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ): unknown {
    const fn = this.kernel.emSimulate;
    if (typeof fn !== "function") {
      throw new Error(
        "emSimulate is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson, optionsJson);
  }

  /**
   * Thin-wire MoM antenna analysis: Z_in/S11 sweep, in-band resonance,
   * peak gain, and predicted receipt claims. Inputs are JSON strings
   * (AntennaSpec, named parameter bindings, options with the required
   * frequency band); see `vcad-kernel-antenna`.
   */
  antennaAnalyze(
    specJson: string,
    paramsJson: string,
    optionsJson: string,
  ): unknown {
    const fn = this.kernel.antennaAnalyze;
    if (typeof fn !== "function") {
      throw new Error(
        "antennaAnalyze is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson, optionsJson);
  }

  /**
   * Forward 2D TM FDTD photonics run: rect-composed device, slab-mode
   * source, transmission spectrum, and predicted receipt claims. Inputs
   * are JSON strings (device spec, options); see `vcad-kernel-photonics`.
   */
  photonicsSimulate(specJson: string, optionsJson: string): unknown {
    const fn = this.kernel.photonicsSimulate;
    if (typeof fn !== "function") {
      throw new Error(
        "photonicsSimulate is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, optionsJson);
  }

  /**
   * Monte Carlo neutron shielding run: dose at detector shells with
   * statistical error bars and predicted receipt claims. Inputs are JSON
   * strings (ShieldSpec, named parameter bindings); see
   * `vcad-kernel-neutronics`.
   */
  neutronicsSimulate(specJson: string, paramsJson: string): unknown {
    const fn = this.kernel.neutronicsSimulate;
    if (typeof fn !== "function") {
      throw new Error(
        "neutronicsSimulate is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(specJson, paramsJson);
  }

  topologyOptimizeBox(
    min: [number, number, number],
    max: [number, number, number],
    spec: TopoOptSpec,
  ): TopoOptResult {
    const fn = this.kernel.topologyOptimizeBox;
    if (typeof fn !== "function") {
      throw new Error(
        "topologyOptimizeBox is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(
      JSON.stringify(spec),
      min[0],
      min[1],
      min[2],
      max[0],
      max[1],
      max[2],
    ) as TopoOptResult;
  }

  /**
   * SIMP topology optimization inside a closed evaluated mesh: the mesh's
   * interior becomes the design domain, so material only appears where the
   * original part had volume ("lightweight this bracket").
   */
  topologyOptimizeMesh(mesh: TriangleMesh, spec: TopoOptSpec): TopoOptResult {
    const fn = this.kernel.topologyOptimizeMesh;
    if (typeof fn !== "function") {
      throw new Error(
        "topologyOptimizeMesh is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(JSON.stringify(spec), mesh.positions, mesh.indices) as TopoOptResult;
  }

  /**
   * Static structural analysis of a solid box under the spec's loads and
   * supports. Resolution is the fidelity dial: ~32 answers fast (the
   * `predicted` tier), 64–96 is the trusted `verified` tier — same solver,
   * finer grid.
   */
  analyzeStaticsBox(
    min: [number, number, number],
    max: [number, number, number],
    spec: StaticAnalysisSpec,
  ): StaticAnalysisResult {
    const fn = this.kernel.analyzeStaticsBox;
    if (typeof fn !== "function") {
      throw new Error(
        "analyzeStaticsBox is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(
      JSON.stringify(spec),
      min[0],
      min[1],
      min[2],
      max[0],
      max[1],
      max[2],
    ) as StaticAnalysisResult;
  }

  /**
   * Static structural analysis inside a closed evaluated mesh: the mesh
   * interior is voxelized and solved under the given loads/supports.
   */
  analyzeStaticsMesh(
    mesh: TriangleMesh,
    spec: StaticAnalysisSpec,
  ): StaticAnalysisResult {
    const fn = this.kernel.analyzeStaticsMesh;
    if (typeof fn !== "function") {
      throw new Error(
        "analyzeStaticsMesh is not exported by this kernel WASM build — rebuild packages/kernel-wasm",
      );
    }
    return fn(
      JSON.stringify(spec),
      mesh.positions,
      mesh.indices,
    ) as StaticAnalysisResult;
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

  /**
   * Import a URDF (Unified Robot Description Format) file. Returns a
   * JSON-encoded vcad `Document` that the caller deserialises with
   * `Document.fromJson` (or hands directly to the document store).
   *
   * Mesh references inside the URDF can't be resolved from the browser
   * filesystem, so any `<mesh>` falls back to a 1cm placeholder cube.
   * Joint topology and authored `<inertial>` properties still flow
   * through unchanged, so simulation behaves like the real robot to
   * first order.
   */
  importUrdf(data: ArrayBuffer): string {
    const bytes = new Uint8Array(data);
    if (typeof this.kernel.importUrdfBuffer !== "function") {
      throw new Error(
        "URDF import not available — kernel WASM was built without urdf support",
      );
    }
    return this.kernel.importUrdfBuffer(bytes);
  }

  /** Export a projected view to DXF format.
   *
   * Returns the DXF file content as a Uint8Array.
   */
  exportDrawingToDxf(view: ProjectedView): Uint8Array {
    const json = JSON.stringify(view);
    return this.kernel.exportProjectedViewToDxf(json);
  }

  /** Offset (stepped) section of a mesh. Returns null when the kernel lacks
   * the binding or the inputs fail to parse. */
  offsetSectionMesh(
    mesh: { positions: Float32Array; indices: Uint32Array },
    plane: OffsetSectionPlane,
    hatch?: { spacing: number; angle: number },
  ): SectionView | null {
    if (!this.kernel.offsetSectionMesh) return null;
    return this.kernel.offsetSectionMesh(
      mesh,
      JSON.stringify(plane),
      hatch ? JSON.stringify(hatch) : undefined,
    );
  }

  /** Render a title block (kernel-drawn, matches the PDF output). */
  renderTitleBlock(fields: TitleBlockFields): RenderedBlock | null {
    if (!this.kernel.renderTitleBlock) return null;
    return this.kernel.renderTitleBlock(JSON.stringify(fields));
  }

  /** Render a BOM table (kernel-drawn, matches the PDF output). */
  renderBomTable(rows: BomRow[]): RenderedBlock | null {
    if (!this.kernel.renderBomTable) return null;
    return this.kernel.renderBomTable(JSON.stringify(rows));
  }

  /** Compose a drawing sheet and export it as PDF bytes. Returns null when
   * the kernel lacks the binding. */
  drawingSheetToPdf(spec: DrawingSheetSpec): Uint8Array | null {
    if (!this.kernel.drawingSheetToPdf) return null;
    return this.kernel.drawingSheetToPdf(JSON.stringify(spec));
  }

  /**
   * Find the first visible sheet-metal part in `doc` and rebuild its op chain.
   *
   * Resolves the chain tip through any Translate/Rotate/Scale wrapper so a
   * POSITIONED bracket (e.g. `Translate(child: EdgeFlange)`) is found, not just
   * a bare root — the same recognition the render path uses. The chain is
   * placement-independent (flat-2D), so these pure queries don't need the
   * transform. Returns `null` when the document has no sheet-metal part.
   */
  private firstSheetMetalChain(
    doc: Document,
  ): ReturnType<typeof buildSheetMetalChain> {
    for (const entry of doc.roots) {
      if (entry.visible === false) continue;
      const sm = findSheetMetalChainRoot(entry.root, doc.nodes);
      if (!sm) continue;
      const chain = buildSheetMetalChain(sm.root, doc.nodes);
      if (chain) return chain;
    }
    return null;
  }

  /**
   * Estimate the manufacturing cost of the sheet-metal part in `doc`.
   *
   * Finds the first sheet-metal root, rebuilds its op chain, and asks the
   * kernel for a line-itemed breakdown using `rates` (or
   * {@link DEFAULT_COST_RATES} when omitted). Returns `null` if the document
   * has no sheet-metal part. Pure query — does not evaluate meshes.
   */
  costSheetMetal(
    doc: Document,
    rates?: SheetMetalCostRates,
    quantity = 1,
  ): SheetMetalCostResult | null {
    const chain = this.firstSheetMetalChain(doc);
    if (!chain) return null;
    return costSheetMetalChain(
      chain,
      this.kernel as unknown as Parameters<typeof costSheetMetalChain>[1],
      rates,
      quantity,
    );
  }

  /** Nest part footprints on stock sheets (bottom-left fill
   *  decreasing). Returns placements + per-sheet utilization. */
  nestSheetMetalParts(
    parts: SheetMetalPartFootprint[],
    params?: SheetMetalNestingParams,
  ): SheetMetalNestingResult {
    return runNestSheetMetalParts(
      parts,
      this.kernel as unknown as Parameters<typeof runNestSheetMetalParts>[1],
      params,
    );
  }

  /** Compute a feasible bend sequence (outermost-first) for the
   *  sheet-metal part in `doc`. Returns `null` if there is none. */
  sheetMetalSequence(doc: Document): SheetMetalBendStep[] | null {
    const chain = this.firstSheetMetalChain(doc);
    if (!chain) return null;
    return runSheetMetalSequence(
      chain,
      this.kernel as unknown as Parameters<typeof runSheetMetalSequence>[1],
    );
  }

  /** Return the kernel's curated sheet-metal materials registry. */
  getSheetMetalMaterials(): SheetMetalMaterial[] {
    return readSheetMetalMaterials(
      this.kernel as unknown as Parameters<typeof readSheetMetalMaterials>[0],
    );
  }

  /** Return the kernel's curated bend table. */
  getSheetMetalBendTable(): SheetMetalBendTable {
    return readSheetMetalBendTable(
      this.kernel as unknown as Parameters<typeof readSheetMetalBendTable>[0],
    );
  }

  /** Return a built-in fab-service bending catalog (e.g. `"sendcutsend"`):
   *  per-material/thickness fixed radii, K-factors, die widths, min flange
   *  sizes, and relief depths. Throws on unknown ids. */
  getSheetMetalShopCatalog(shopId: string): SheetMetalShopCatalog {
    return readSheetMetalShopCatalog(
      this.kernel as unknown as Parameters<typeof readSheetMetalShopCatalog>[0],
      shopId,
    );
  }

  /**
   * Run sheet-metal manufacturability against a shop profile.
   *
   * Finds the first sheet-metal root in `doc`, rebuilds its op chain, and
   * asks the kernel for structured violations vs. `shop`. `shop` is a
   * profile object, a built-in catalog id string (e.g. `"sendcutsend"`),
   * or omitted (→ the chain's own shop profile if set, else generic).
   * Returns `null` if the document has no sheet-metal part. Pure query —
   * does not evaluate meshes or touch the scene cache.
   */
  checkSheetMetal(
    doc: Document,
    shop?: SheetMetalShopProfile | string,
  ): SheetMetalCheckResult | null {
    const chain = this.firstSheetMetalChain(doc);
    if (!chain) return null;
    return checkSheetMetalManufacturability(
      chain,
      this.kernel as unknown as Parameters<
        typeof checkSheetMetalManufacturability
      >[1],
      shop,
    );
  }

  /**
   * Export the document's FOLDED sheet-metal body as a STEP AP214 string.
   *
   * Finds the first sheet-metal root, rebuilds its op chain, and asks the
   * kernel for the folded solid with true cylindrical bend faces (radii/K
   * from the chain's shop profile when one is set) — the zero-data-entry
   * upload path for fab services with a 3D pipeline. Returns `null` when
   * the document has no sheet-metal part; throws on kernel errors (e.g.
   * hems/closed folds, which the folded body cannot represent).
   */
  foldedSheetMetalStep(doc: Document): string | null {
    const chain = this.firstSheetMetalChain(doc);
    if (!chain) return null;
    return buildFoldedSheetMetalStep(
      chain,
      this.kernel as unknown as Parameters<
        typeof buildFoldedSheetMetalStep
      >[1],
    );
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
        segments: segments.map(convertSegment),
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
        segments: segments.map(convertSegment),
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

  /**
   * Evaluate a preview sweep without adding to the document. Mirrors the
   * shape of `evaluateExtrudePreview` so the new continuous-preview hook can
   * dispatch by op kind. The path discriminant matches `addSweep`'s
   * `PathCurve` shape so callers can pass the same value to both.
   */
  evaluateSweepPreview(
    origin: Vec3,
    xDir: Vec3,
    yDir: Vec3,
    segments: SketchSegment2D[],
    path:
      | { type: "Line"; start: Vec3; end: Vec3 }
      | { type: "Helix"; radius: number; pitch: number; height: number; turns: number },
  ): TriangleMesh | null {
    if (segments.length === 0) return null;

    try {
      const profile = {
        origin: [origin.x, origin.y, origin.z],
        x_dir: [xDir.x, xDir.y, xDir.z],
        y_dir: [yDir.x, yDir.y, yDir.z],
        segments: segments.map(convertSegment),
      };

      const profileJson = JSON.stringify(profile);
      const solid =
        path.type === "Line"
          ? this.kernel.Solid.sweepLine(
              profileJson,
              new Float64Array([path.start.x, path.start.y, path.start.z]),
              new Float64Array([path.end.x, path.end.y, path.end.z]),
            )
          : this.kernel.Solid.sweepHelix(
              profileJson,
              path.radius,
              path.pitch,
              path.height,
              path.turns,
            );
      const meshData = solid.getMesh();

      return {
        positions: new Float32Array(meshData.positions),
        indices: new Uint32Array(meshData.indices),
      };
    } catch (e) {
      // See evaluateExtrudePreview — silent catches here poison wasm borrows.
      console.warn("[engine] evaluateSweepPreview failed:", e);
      return null;
    }
  }

  /**
   * Evaluate a preview loft across a list of profiles. Mirrors `addLoft`'s
   * profile shape so the continuous-preview hook can pass the same array.
   */
  evaluateLoftPreview(
    profiles: Array<{
      plane: { x_dir: Vec3; y_dir: Vec3 };
      origin: Vec3;
      segments: SketchSegment2D[];
    }>,
    closed?: boolean,
  ): TriangleMesh | null {
    if (profiles.length < 2) return null;

    try {
      const profileObjs = profiles.map((p) => ({
        origin: [p.origin.x, p.origin.y, p.origin.z],
        x_dir: [p.plane.x_dir.x, p.plane.x_dir.y, p.plane.x_dir.z],
        y_dir: [p.plane.y_dir.x, p.plane.y_dir.y, p.plane.y_dir.z],
        segments: p.segments.map((seg) => {
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
      }));

      const solid = this.kernel.Solid.loft(JSON.stringify(profileObjs), closed ?? false);
      const meshData = solid.getMesh();

      return {
        positions: new Float32Array(meshData.positions),
        indices: new Uint32Array(meshData.indices),
      };
    } catch (e) {
      // See evaluateExtrudePreview — silent catches here poison wasm borrows.
      console.warn("[engine] evaluateLoftPreview failed:", e);
      return null;
    }
  }
}
