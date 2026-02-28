// Types
export type {
  PrimitiveKind,
  BooleanType,
  SketchPlane,
  AxisAlignedPlane,
  ArbitraryPlane,
  FaceInfo,
  PrimitivePartInfo,
  BooleanPartInfo,
  ExtrudePartInfo,
  RevolvePartInfo,
  SweepPartInfo,
  LoftPartInfo,
  ImportedMeshPartInfo,
  FilletPartInfo,
  ChamferPartInfo,
  ShellPartInfo,
  LinearPatternPartInfo,
  CircularPatternPartInfo,
  MirrorPartInfo,
  TextPartInfo,
  PcbBoardPartInfo,
  EmbroideryPatternPartInfo,
  StitchPartInfo,
  PartInfo,
  ToolMode,
  TransformMode,
  Theme,
  ConstraintTool,
  ConstraintStatus,
  SketchState,
} from "./types.js";

export {
  isPrimitivePart,
  isBooleanPart,
  isExtrudePart,
  isRevolvePart,
  isSweepPart,
  isLoftPart,
  isImportedMeshPart,
  isFilletPart,
  isChamferPart,
  isShellPart,
  isLinearPatternPart,
  isCircularPatternPart,
  isMirrorPart,
  isTextPart,
  isPcbBoardPart,
  isEmbroideryPatternPart,
  isStitchPart,
  isStitchEligible,
  getSketchPlaneDirections,
  isAxisAlignedPlane,
  computePlaneFromFace,
  getSketchPlaneName,
  formatDirection,
  negateDirection,
} from "./types.js";

// Stores
export { useDocumentStore, getNodePcb, getPcbNodeIds, getNodeEmbroideryDesign } from "./stores/document-store.js";
export type { VcadFile, DocumentState, PcbCreateOptions } from "./stores/document-store.js";

export type { FeatureInput } from "./stores/feature-input.js";

export { useCrdtDocumentStore, f64, vec3, bool, str, featureRef } from "./stores/crdt-document-store.js";
export type { CrdtDocumentState, CrdtValue } from "./stores/crdt-document-store.js";

export { useUiStore } from "./stores/ui-store.js";
export type { UiState, MaterialPreview, RenderMode, RaytraceQuality, RaytraceDebugMode, ToolbarTab } from "./stores/ui-store.js";

export { useSketchStore } from "./stores/sketch-store.js";
export type { SketchStore, ProfileSnapshot, SketchExitStatus } from "./stores/sketch-store.js";

export { useEngineStore } from "./stores/engine-store.js";
export type { EngineState } from "./stores/engine-store.js";

export { useSimulationStore } from "./stores/simulation-store.js";
export type {
  SimulationState,
  SimulationMode,
  ActionType,
  JointState,
  SimulationObservation,
} from "./stores/simulation-store.js";

export { useCoreElectronicsStore } from "./stores/electronics-store.js";
export type {
  CoreElectronicsState,
  PcbTool,
  SchTool,
  ElectronicsSelection,
} from "./stores/electronics-store.js";

// Commands
export { createCommandRegistry } from "./commands.js";
export type { Command, CommandRegistry, CommandActions } from "./commands.js";
export { createDefaultCommandActions } from "./command-actions.js";

// Part labels
export { PART_GLYPHS, getPartGlyph } from "./part-labels.js";

// Export utilities
export { exportStlBuffer, exportStlBlob } from "./utils/export-stl.js";
export { exportGltfBuffer, exportGltfBlob } from "./utils/export-gltf.js";
export { exportStepBuffer, exportStepBlob } from "./utils/export-step.js";
export { serializeDocument, parseVcadFile, deriveParts } from "./utils/save-load.js";
export { documentToLoon } from "./utils/document-to-loon.js";
export type { VcadFile as VcadFileFormat } from "./utils/save-load.js";
export { computeVolume, computeMass, formatMass, formatVolume } from "./utils/geometry.js";
export { parseStl } from "./utils/import-stl.js";

// Engine lifecycle
export { initEngineLifecycle } from "./engine-lifecycle.js";
export type { EngineLifecycleOptions } from "./engine-lifecycle.js";

// Re-export engine initialization
export { Engine } from "@vcad/engine";
export type {
  EvaluatedScene,
  EvaluatedPart,
  TriangleMesh,
  ProjectedView,
  ProjectedEdge,
  BoundingBox2D,
  RenderedDimension,
  RenderedText,
  RenderedArrow,
  RenderedArc,
  DetailView,
  DetailViewParams,
} from "@vcad/engine";

// Logger
export { logger, LogLevel, LogSource } from "./logger.js";
export type {
  LogEntry,
  LogLevelName,
  LogSourceName,
  LogSubscriber,
} from "./logger.js";

// Changelog
export {
  changelog,
  CURRENT_VERSION,
  getEntriesSince,
  getEntriesForVersion,
  getEntriesByCategory,
  getEntriesForTool,
} from "./changelog/index.js";
export type {
  Changelog,
  ChangelogEntry,
  ChangelogCategory,
} from "./changelog/index.js";
