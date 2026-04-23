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
export type {
  UiState,
  MaterialPreview,
  RenderMode,
  RaytraceQuality,
  RaytraceDebugMode,
  ToolbarTab,
  SidebarPane,
  InspectorTarget,
  FollowMode,
} from "./stores/ui-store.js";

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

export { useChatStore } from "./stores/chat-store.js";
export type {
  ChatState,
  ChatMessage,
  ChatMessageStatus,
  SelectionContext,
  ToolCallInfo,
  MessagePart,
  ChatAttachment,
  ChatUsageError,
  HydrateHandler,
} from "./stores/chat-store.js";

export {
  useParticipantStore,
  makeAiParticipant,
  ensureAiParticipant,
  LOCAL_PARTICIPANT_ID,
  AI_PARTICIPANT_ID,
} from "./stores/participant-store.js";
export type {
  Participant,
  ParticipantKind,
  ParticipantState,
} from "./stores/participant-store.js";

// Billing
export {
  TIERS,
  PURCHASABLE_TIERS,
  getTier,
  tierFromStripeLookupKey,
  parseTier,
  formatTokens,
} from "./billing/tiers.js";
export type { Tier, TierId, PaidTierId } from "./billing/tiers.js";

export {
  useBillingStore,
  totalTokensUsed,
  usageFraction,
  usageSeverity,
  parseUsageResponse,
} from "./stores/billing-store.js";
export type { BillingState, UsageSnapshot } from "./stores/billing-store.js";

export { useCoreElectronicsStore } from "./stores/electronics-store.js";
export type {
  CoreElectronicsState,
  PcbTool,
  SchTool,
  ElectronicsSelection,
} from "./stores/electronics-store.js";

// Commands (palette / menus)
export {
  createCommandRegistry,
  COMMAND_CATEGORIES,
  CATEGORY_LABELS,
  CATEGORY_ICON_COLORS,
} from "./commands.js";
export type { Command, CommandRegistry, CommandActions, CommandCategory } from "./commands.js";
export { createDefaultCommandActions } from "./command-actions.js";

// Keybinding registry (shared with Rust via kernel-wasm)
export type {
  Chord,
  Key,
  WhenBits,
  WhenInputs,
  AppMode as KeybindingMode,
  CommandView as KeybindingCommandView,
} from "./keybindings/index.js";
export {
  WHEN,
  KeybindingRegistry,
  chordFromEvent,
  formatChord,
  isMac,
  buildWhenContext,
  isInputFocused,
  getKeybindingRegistry,
  getKeybindingRegistrySync,
} from "./keybindings/index.js";

// AI Tool Registry (CRUD)
export { commandRegistry, executeCrud, HIGH_LEVEL_TOOLS_SYSTEM_PROMPT_APPENDIX } from "./commands/index.js";
export type { ToolSchemaEntry, ExecutionResult, ExecutionDisplay, SummarySegment, AnthropicTool } from "./commands/index.js";

// Part labels
export { PART_GLYPHS, getPartGlyph } from "./part-labels.js";

// Export utilities
export { exportStlBuffer, exportStlBlob } from "./utils/export-stl.js";
export { exportGltfBuffer, exportGltfBlob } from "./utils/export-gltf.js";
export { exportStepBuffer, exportStepBlob } from "./utils/export-step.js";
export {
  serializeDocument,
  parseVcadFile,
  deriveParts,
  getDocumentForDisplay,
  buildVcadFileFromState,
} from "./utils/save-load.js";
export { documentToLoon } from "./utils/document-to-loon.js";
export type {
  VcadFile as VcadFileFormat,
  VcadFileCrdt,
  VcadFileLoon,
  VcadFileLegacy,
} from "./utils/save-load.js";
export { computeVolume, computeMass, formatMass, formatVolume } from "./utils/geometry.js";
export { parseStl } from "./utils/import-stl.js";

// Engine lifecycle
export { initEngineLifecycle } from "./engine-lifecycle.js";
export type { EngineLifecycleOptions } from "./engine-lifecycle.js";

// WASM singleton — prevents double-instantiation
export { getKernelWasm, getKernelWasmSync, primeKernelWasm } from "./wasm-singleton.js";

// Kernel-backed sketch math (projection, snap, hit-test, shape builders)
export {
  getPlaneBasis,
  worldToSketch,
  sketchToWorld,
  intersectRay,
  snapPoint,
  hitTestSegments,
  buildRectangle,
  buildCircle,
} from "./sketch-math.js";
export type { PlaneBasis, SnapOptions, SnapResult } from "./sketch-math.js";

// Camera framing math (kernel Z-up)
export {
  SNAP_VIEWS,
  isSnapView,
  bboxCenter,
  bboxSize,
  bboxMaxDim,
  expandBboxFromPositions,
  clampFramingDistance,
  frameBbox,
  defaultCameraGoal,
  kernelToDisplay,
  displayToKernel,
} from "./camera-framing.js";
export type { Vec3, Bbox, CameraGoal, SnapView } from "./camera-framing.js";

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
