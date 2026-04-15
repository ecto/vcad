import { useMemo, type ComponentType, type ReactNode } from "react";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Cylinder } from "@phosphor-icons/react/dist/ssr/Cylinder";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { Unite } from "@phosphor-icons/react/dist/ssr/Unite";
import { Subtract } from "@phosphor-icons/react/dist/ssr/Subtract";
import { Intersect } from "@phosphor-icons/react/dist/ssr/Intersect";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { ArrowsOut } from "@phosphor-icons/react/dist/ssr/ArrowsOut";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { PlusSquare } from "@phosphor-icons/react/dist/ssr/PlusSquare";
import { LinkSimple } from "@phosphor-icons/react/dist/ssr/LinkSimple";
import { Cube as Cube3D } from "@phosphor-icons/react/dist/ssr/Cube";
import { Blueprint } from "@phosphor-icons/react/dist/ssr/Blueprint";
import { Download } from "@phosphor-icons/react/dist/ssr/Download";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Octagon } from "@phosphor-icons/react/dist/ssr/Octagon";
import { CubeTransparent } from "@phosphor-icons/react/dist/ssr/CubeTransparent";
import { DotsThree } from "@phosphor-icons/react/dist/ssr/DotsThree";
import { ArrowsHorizontal } from "@phosphor-icons/react/dist/ssr/ArrowsHorizontal";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { Pause } from "@phosphor-icons/react/dist/ssr/Pause";
import { Stop } from "@phosphor-icons/react/dist/ssr/Stop";
import { FastForward } from "@phosphor-icons/react/dist/ssr/FastForward";
import { Printer } from "@phosphor-icons/react/dist/ssr/Printer";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import { Path } from "@phosphor-icons/react/dist/ssr/Path";
import { Circuitry } from "@phosphor-icons/react/dist/ssr/Circuitry";
import { Scissors } from "@phosphor-icons/react/dist/ssr/Scissors";
import { TextT } from "@phosphor-icons/react/dist/ssr/TextT";

import {
  useDocumentStore,
  useUiStore,
  useSketchStore,
  useEngineStore,
  useSimulationStore,
  exportStlBlob,
  exportGltfBlob,
  exportStepBlob,
  isStitchEligible,
  getPcbNodeIds,
  type ToolbarTab,
  type PrimitiveKind,
  type BooleanType,
} from "@vcad/core";
import { TAB_COLORS } from "@/components/ui/toolbar-constants";
import { useDrawingStore } from "@/stores/drawing-store";
import { useSlicerStore } from "@/stores/slicer-store";
import { useCamStore } from "@/stores/cam-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";
import { useOutputStore, estimatePrice } from "@/stores/output-store";
import { useNotificationStore } from "@/stores/notification-store";
import { useOnboardingStore, type GuidedFlowStep } from "@/stores/onboarding-store";
import { analytics } from "@/lib/analytics";
import { downloadBlob } from "@/lib/download";

export type ToolIcon = ComponentType<{
  size?: number;
  weight?: "regular" | "fill";
  className?: string;
}>;

/** A single tool button, shared between desktop ToolPalette and MobileToolPalette. */
export interface ToolDef {
  id: string;
  tab: ToolbarTab;
  label: string;
  tooltip: string;
  icon: ToolIcon;
  shortcut?: string;
  enabled: boolean;
  active?: boolean;
  pulse?: boolean;
  iconColor?: string;
  /** Optional override classes for the desktop ToolbarButton wrapper. */
  className?: string;
  labelClassName?: string;
  onClick: () => void;
}

export interface ToolTabMeta {
  id: ToolbarTab;
  label: string;
  icon: ToolIcon;
}

export const ALL_TABS: ToolTabMeta[] = [
  { id: "create", label: "Create", icon: Cube },
  { id: "transform", label: "Transform", icon: ArrowsOutCardinal },
  { id: "combine", label: "Combine", icon: Unite },
  { id: "modify", label: "Modify", icon: Circle },
  { id: "assembly", label: "Assembly", icon: Package },
  { id: "simulate", label: "Simulate", icon: Play },
  { id: "build", label: "Export", icon: Export },
];

const PRIMITIVES: { kind: PrimitiveKind; icon: ToolIcon; label: string }[] = [
  { kind: "cube", icon: Cube, label: "Box" },
  { kind: "cylinder", icon: Cylinder, label: "Cylinder" },
  { kind: "sphere", icon: Globe, label: "Sphere" },
];

const BOOLEANS: {
  type: BooleanType;
  icon: ToolIcon;
  label: string;
  shortcut: string;
}[] = [
  { type: "union", icon: Unite, label: "Union", shortcut: "⌘⇧U" },
  { type: "difference", icon: Subtract, label: "Difference", shortcut: "⌘⇧D" },
  { type: "intersection", icon: Intersect, label: "Intersection", shortcut: "⌘⇧I" },
];

/** Dispatch helper for the modify dialogs, which are owned by ToolDialogs. */
function dispatch(name: string) {
  window.dispatchEvent(new CustomEvent(name));
}

/**
 * Shared hook that produces the live tool definitions for both the desktop
 * ToolPalette and the mobile MobileToolPalette. Every interactive control in
 * the tool palette maps to a ToolDef — the consumers only have to decide how
 * to render it.
 *
 * Tab-level extras (like the simulation speed slider) are returned via
 * `renderExtras`, which is a render-prop so callers can inject their own
 * wrapper styling.
 */
export function useToolDefinitions(): {
  byTab: Record<ToolbarTab, ToolDef[]>;
  tabs: ToolTabMeta[];
  renderSimulateExtras: (opts?: { compact?: boolean }) => ReactNode;
} {
  // Document
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const applyBoolean = useDocumentStore((s) => s.applyBoolean);
  const createPartDef = useDocumentStore((s) => s.createPartDef);
  const document = useDocumentStore((s) => s.document);
  const parts = useDocumentStore((s) => s.parts);

  // UI
  const select = useUiStore((s) => s.select);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const transformMode = useUiStore((s) => s.transformMode);
  const setTransformMode = useUiStore((s) => s.setTransformMode);

  // Sketch
  const enterSketchMode = useSketchStore((s) => s.enterSketchMode);
  const enterFaceSelectionMode = useSketchStore((s) => s.enterFaceSelectionMode);
  const sketchActive = useSketchStore((s) => s.active);
  const faceSelectionMode = useSketchStore((s) => s.faceSelectionMode);

  // Drawing
  const viewMode = useDrawingStore((s) => s.viewMode);
  const setViewMode = useDrawingStore((s) => s.setViewMode);

  // Engine
  const scene = useEngineStore((s) => s.scene);
  const hasSceneParts = Boolean(scene?.parts?.length);

  // Simulation
  const simMode = useSimulationStore((s) => s.mode);
  const physicsAvailable = useSimulationStore((s) => s.physicsAvailable);
  const playbackSpeed = useSimulationStore((s) => s.playbackSpeed);
  const playSim = useSimulationStore((s) => s.play);
  const pauseSim = useSimulationStore((s) => s.pause);
  const stopSim = useSimulationStore((s) => s.stop);
  const stepSim = useSimulationStore((s) => s.step);
  const setPlaybackSpeed = useSimulationStore((s) => s.setPlaybackSpeed);

  // Onboarding (guided flow pulses)
  const guidedFlowActive = useOnboardingStore((s) => s.guidedFlowActive);
  const guidedFlowStep = useOnboardingStore((s) => s.guidedFlowStep);
  const advanceGuidedFlow = useOnboardingStore((s) => s.advanceGuidedFlow);

  // Output
  const openQuotePanel = useOutputStore((s) => s.openQuotePanel);
  const selectedMaterial = useOutputStore((s) => s.selectedMaterial);
  const estimatedPrice = estimatePrice(scene, selectedMaterial);

  // Derived selection state
  const hasSelection = selectedPartIds.size > 0;
  const hasTwoSelected = selectedPartIds.size === 2;
  const hasPartDefs = Boolean(
    document.partDefs && Object.keys(document.partDefs).length > 0,
  );
  const hasInstances = Boolean(document.instances && document.instances.length > 0);
  const hasJoints = Boolean(document.joints && document.joints.length > 0);
  const hasOnePartSelected =
    selectedPartIds.size === 1 && parts.some((p) => selectedPartIds.has(p.id));
  const selectedInstanceIds = Array.from(selectedPartIds).filter((id) =>
    document.instances?.some((i) => i.id === id),
  );
  const hasTwoInstancesSelected = selectedInstanceIds.length === 2;
  const selectedPartId = hasOnePartSelected
    ? Array.from(selectedPartIds).find((id) => parts.some((p) => p.id === id)) ?? null
    : null;
  const selectedPartStitchEligible = selectedPartId
    ? (() => {
        const p = parts.find((pp) => pp.id === selectedPartId);
        return p ? isStitchEligible(p) : false;
      })()
    : false;

  void hasInstances;

  const buildTooltip = !hasSceneParts
    ? "Build (add geometry first)"
    : estimatedPrice
      ? `Build (~$${estimatedPrice.toFixed(0)})`
      : "Build";

  const byTab = useMemo<Record<ToolbarTab, ToolDef[]>>(() => {
    const color = (tab: ToolbarTab) => TAB_COLORS[tab];
    const shouldPulse = (step: GuidedFlowStep, extra: boolean = true): boolean =>
      guidedFlowActive && guidedFlowStep === step && extra;

    function handleAddPrimitive(kind: PrimitiveKind) {
      const partId = addPrimitive(kind);
      select(partId);
      setTransformMode("translate");
      analytics.primitiveAdded(kind);
      if (guidedFlowActive) {
        if (guidedFlowStep === "add-cube" && kind === "cube") advanceGuidedFlow();
        else if (guidedFlowStep === "add-cylinder" && kind === "cylinder")
          advanceGuidedFlow();
      }
    }

    function handleBoolean(type: BooleanType) {
      if (!hasTwoSelected) return;
      const ids = Array.from(selectedPartIds);
      const newId = applyBoolean(type, ids[0]!, ids[1]!);
      if (newId) select(newId);
      analytics.booleanApplied(type);
      if (guidedFlowActive && guidedFlowStep === "subtract" && type === "difference") {
        advanceGuidedFlow();
      }
    }

    function handleCreatePartDef() {
      if (!hasOnePartSelected) return;
      const partId = Array.from(selectedPartIds)[0]!;
      const partDefId = createPartDef(partId);
      if (partDefId) {
        const instance = document.instances?.find((i) => i.partDefId === partDefId);
        if (instance) select(instance.id);
      }
    }

    const create: ToolDef[] = [
      ...PRIMITIVES.map(({ kind, icon, label }) => ({
        id: `create-${kind}`,
        tab: "create" as const,
        label,
        tooltip: `Add ${label}`,
        icon,
        enabled: !sketchActive,
        pulse:
          (kind === "cube" && shouldPulse("add-cube")) ||
          (kind === "cylinder" && shouldPulse("add-cylinder")),
        iconColor: color("create"),
        onClick: () => handleAddPrimitive(kind),
      })),
      {
        id: "create-sketch",
        tab: "create",
        label: "Sketch",
        tooltip: "New Sketch (S)",
        icon: PencilSimple,
        shortcut: "S",
        enabled: !sketchActive,
        active: faceSelectionMode,
        iconColor: color("create"),
        onClick: () => {
          analytics.sketchStarted();
          if (parts.length > 0) enterFaceSelectionMode();
          else enterSketchMode("XY");
        },
      },
      {
        id: "create-text",
        tab: "create",
        label: "Text",
        tooltip: "Add Text",
        icon: TextT,
        enabled: !sketchActive,
        iconColor: color("create"),
        onClick: () => dispatch("vcad:open-text-dialog"),
      },
      {
        id: "create-pcb",
        tab: "create",
        label: "PCB",
        tooltip: "Add PCB Board",
        icon: Circuitry,
        enabled: !sketchActive,
        iconColor: color("create"),
        onClick: () => dispatch("vcad:open-pcb-dialog"),
      },
    ];

    const transform: ToolDef[] = [
      {
        id: "transform-move",
        tab: "transform",
        label: "Move",
        tooltip: !hasSelection ? "Move (select a part)" : "Move (M)",
        icon: ArrowsOutCardinal,
        shortcut: "M",
        enabled: hasSelection && viewMode !== "2d",
        active: hasSelection && transformMode === "translate",
        iconColor: color("transform"),
        onClick: () => setTransformMode("translate"),
      },
      {
        id: "transform-rotate",
        tab: "transform",
        label: "Rotate",
        tooltip: !hasSelection ? "Rotate (select a part)" : "Rotate (R)",
        icon: ArrowsClockwise,
        shortcut: "R",
        enabled: hasSelection && viewMode !== "2d",
        active: hasSelection && transformMode === "rotate",
        iconColor: color("transform"),
        onClick: () => setTransformMode("rotate"),
      },
      {
        id: "transform-scale",
        tab: "transform",
        label: "Scale",
        tooltip: !hasSelection ? "Scale (select a part)" : "Scale (S)",
        icon: ArrowsOut,
        shortcut: "S",
        enabled: hasSelection && viewMode !== "2d",
        active: hasSelection && transformMode === "scale",
        iconColor: color("transform"),
        onClick: () => setTransformMode("scale"),
      },
    ];

    const combine: ToolDef[] = BOOLEANS.map(({ type, icon, label, shortcut }) => ({
      id: `combine-${type}`,
      tab: "combine" as const,
      label,
      tooltip: !hasTwoSelected
        ? `${label} (select 2 parts)`
        : `${label} (${shortcut})`,
      icon,
      shortcut,
      enabled: hasTwoSelected,
      pulse: type === "difference" && shouldPulse("subtract"),
      iconColor: color("combine"),
      onClick: () => handleBoolean(type),
    }));

    const modifyEnabled = hasOnePartSelected && !sketchActive;
    const modify: ToolDef[] = [
      {
        id: "modify-fillet",
        tab: "modify",
        label: "Fillet",
        tooltip: !hasOnePartSelected ? "Fillet (select a part)" : "Fillet",
        icon: Circle,
        enabled: modifyEnabled,
        iconColor: color("modify"),
        onClick: () => dispatch("vcad:apply-fillet"),
      },
      {
        id: "modify-chamfer",
        tab: "modify",
        label: "Chamfer",
        tooltip: !hasOnePartSelected ? "Chamfer (select a part)" : "Chamfer",
        icon: Octagon,
        enabled: modifyEnabled,
        iconColor: color("modify"),
        onClick: () => dispatch("vcad:apply-chamfer"),
      },
      {
        id: "modify-shell",
        tab: "modify",
        label: "Shell",
        tooltip: !hasOnePartSelected ? "Shell (select a part)" : "Shell",
        icon: CubeTransparent,
        enabled: modifyEnabled,
        iconColor: color("modify"),
        onClick: () => dispatch("vcad:apply-shell"),
      },
      {
        id: "modify-pattern",
        tab: "modify",
        label: "Pattern",
        tooltip: !hasOnePartSelected ? "Pattern (select a part)" : "Pattern",
        icon: DotsThree,
        enabled: modifyEnabled,
        iconColor: color("modify"),
        onClick: () => dispatch("vcad:apply-pattern"),
      },
      {
        id: "modify-mirror",
        tab: "modify",
        label: "Mirror",
        tooltip: !hasOnePartSelected ? "Mirror (select a part)" : "Mirror",
        icon: ArrowsHorizontal,
        enabled: modifyEnabled,
        iconColor: color("modify"),
        onClick: () => dispatch("vcad:apply-mirror"),
      },
      {
        id: "modify-stitch",
        tab: "modify",
        label: "Stitch",
        tooltip: !hasOnePartSelected
          ? "Stitch (select a part)"
          : !selectedPartStitchEligible
            ? "Stitch (requires text/extrude/revolve/sweep/loft)"
            : "Stitch",
        icon: Scissors,
        enabled: modifyEnabled && selectedPartStitchEligible,
        iconColor: color("modify"),
        onClick: () => dispatch("vcad:apply-stitch"),
      },
    ];

    const assembly: ToolDef[] = [
      {
        id: "assembly-create-part",
        tab: "assembly",
        label: "Create Part",
        tooltip: !hasOnePartSelected
          ? "Create Part Definition (select a part)"
          : "Create Part Definition",
        icon: Package,
        enabled: hasOnePartSelected && !sketchActive,
        iconColor: color("assembly"),
        onClick: handleCreatePartDef,
      },
      {
        id: "assembly-insert",
        tab: "assembly",
        label: "Insert",
        tooltip: !hasPartDefs
          ? "Insert Instance (create a part def first)"
          : "Insert Instance",
        icon: PlusSquare,
        enabled: hasPartDefs && !sketchActive,
        iconColor: color("assembly"),
        onClick: () => dispatch("vcad:insert-instance"),
      },
      {
        id: "assembly-joint",
        tab: "assembly",
        label: "Joint",
        tooltip: !hasTwoInstancesSelected
          ? "Add Joint (select 2 instances)"
          : "Add Joint",
        icon: LinkSimple,
        enabled: hasTwoInstancesSelected && !sketchActive,
        iconColor: color("assembly"),
        onClick: () => dispatch("vcad:open-joint-dialog"),
      },
    ];

    const simulate: ToolDef[] = [
      {
        id: "simulate-play",
        tab: "simulate",
        label: simMode === "running" ? "Pause" : "Play",
        tooltip: !hasJoints
          ? "Play (add joints to simulate)"
          : simMode === "running"
            ? "Pause Simulation"
            : "Play Simulation",
        icon: simMode === "running" ? Pause : Play,
        enabled: hasJoints && physicsAvailable && !sketchActive,
        active: simMode === "running",
        iconColor: color("simulate"),
        onClick: () => {
          if (simMode === "running") {
            pauseSim();
          } else {
            analytics.physicsSimulationRun();
            playSim();
          }
        },
      },
      {
        id: "simulate-stop",
        tab: "simulate",
        label: "Stop",
        tooltip: !hasJoints ? "Stop (add joints to simulate)" : "Stop Simulation",
        icon: Stop,
        enabled: hasJoints && simMode !== "off" && !sketchActive,
        iconColor: color("simulate"),
        onClick: stopSim,
      },
      {
        id: "simulate-step",
        tab: "simulate",
        label: "Step",
        tooltip: !hasJoints ? "Step (add joints to simulate)" : "Step Simulation",
        icon: FastForward,
        enabled:
          hasJoints && simMode !== "running" && physicsAvailable && !sketchActive,
        iconColor: color("simulate"),
        onClick: stepSim,
      },
    ];

    const build: ToolDef[] = [
      {
        id: "build-build",
        tab: "build",
        label: "Build",
        tooltip: buildTooltip,
        icon: Sparkle,
        enabled: hasSceneParts,
        iconColor: "text-brand",
        className: "bg-brand/10 hover:bg-brand/20 rounded",
        labelClassName: "text-brand font-medium",
        onClick: () => {
          analytics.quotePanelOpened();
          window.dispatchEvent(new CustomEvent("vcad:hero-view"));
          openQuotePanel();
        },
      },
      {
        id: "build-3d",
        tab: "build",
        label: "3D",
        tooltip: "3D View",
        icon: Cube3D,
        enabled: true,
        active: viewMode === "3d",
        iconColor: color("build"),
        onClick: () => setViewMode("3d"),
      },
      {
        id: "build-2d",
        tab: "build",
        label: "2D",
        tooltip: "2D Drawing View",
        icon: Blueprint,
        enabled: true,
        active: viewMode === "2d",
        iconColor: color("build"),
        onClick: () => setViewMode("2d"),
      },
      {
        id: "build-stl",
        tab: "build",
        label: "STL",
        tooltip: hasSceneParts ? "Export STL" : "Export STL (add geometry first)",
        icon: Download,
        enabled: hasSceneParts,
        iconColor: color("build"),
        onClick: () => {
          if (!scene) return;
          const blob = exportStlBlob(scene);
          downloadBlob(blob, "model.stl");
          analytics.documentExported("stl");
          useNotificationStore.getState().addToast("Exported model.stl", "success");
        },
      },
      {
        id: "build-glb",
        tab: "build",
        label: "GLB",
        tooltip: hasSceneParts ? "Export GLB" : "Export GLB (add geometry first)",
        icon: Download,
        enabled: hasSceneParts,
        iconColor: color("build"),
        onClick: () => {
          if (!scene) return;
          const blob = exportGltfBlob(scene);
          downloadBlob(blob, "model.glb");
          analytics.documentExported("glb");
          useNotificationStore.getState().addToast("Exported model.glb", "success");
        },
      },
      {
        id: "build-step",
        tab: "build",
        label: "STEP",
        tooltip: hasSceneParts ? "Export STEP" : "Export STEP (add geometry first)",
        icon: Download,
        enabled: hasSceneParts,
        iconColor: color("build"),
        onClick: () => {
          if (!scene) return;
          try {
            const blob = exportStepBlob(scene);
            downloadBlob(blob, "model.step");
            analytics.documentExported("step");
            useNotificationStore
              .getState()
              .addToast("Exported model.step", "success");
          } catch (e) {
            useNotificationStore
              .getState()
              .addToast(
                e instanceof Error ? e.message : "STEP export failed",
                "error",
              );
          }
        },
      },
      {
        id: "build-print",
        tab: "build",
        label: "Print",
        tooltip: hasSceneParts ? "Open Print Settings" : "Print (add geometry first)",
        icon: Printer,
        enabled: hasSceneParts && !sketchActive,
        iconColor: color("build"),
        onClick: () => {
          analytics.printPanelOpened();
          useSlicerStore.getState().openPrintPanel();
        },
      },
      {
        id: "build-cam",
        tab: "build",
        label: "CAM",
        tooltip: "Open CAM Panel",
        icon: Path,
        enabled: !sketchActive,
        iconColor: color("build"),
        onClick: () => useCamStore.getState().openCamPanel(),
      },
      {
        id: "build-ecad",
        tab: "build",
        label: "ECAD",
        tooltip: "Open Electronics Workspace",
        icon: Circuitry,
        enabled: !sketchActive,
        iconColor: color("build"),
        onClick: () => {
          const doc = useDocumentStore.getState().document;
          const boardIds = getPcbNodeIds(doc);
          if (boardIds.length > 0) {
            useElectronicsStore.getState().enter();
          } else {
            dispatch("vcad:open-pcb-dialog");
          }
        },
      },
      {
        id: "build-emb",
        tab: "build",
        label: "Emb",
        tooltip: "Open Embroidery Panel",
        icon: Scissors,
        enabled: !sketchActive,
        iconColor: color("build"),
        onClick: () => useEmbroideryStore.getState().openPanel(),
      },
    ];

    return {
      create,
      transform,
      combine,
      modify,
      assembly,
      simulate,
      build,
    };
  }, [
    addPrimitive,
    applyBoolean,
    createPartDef,
    document.instances,
    enterFaceSelectionMode,
    enterSketchMode,
    faceSelectionMode,
    guidedFlowActive,
    guidedFlowStep,
    advanceGuidedFlow,
    hasJoints,
    hasOnePartSelected,
    hasPartDefs,
    hasSceneParts,
    hasSelection,
    hasTwoInstancesSelected,
    hasTwoSelected,
    openQuotePanel,
    parts,
    pauseSim,
    physicsAvailable,
    playSim,
    scene,
    select,
    selectedPartIds,
    selectedPartStitchEligible,
    setTransformMode,
    setViewMode,
    simMode,
    sketchActive,
    stepSim,
    stopSim,
    transformMode,
    viewMode,
    buildTooltip,
  ]);

  const renderSimulateExtras = (opts?: { compact?: boolean }): ReactNode => {
    const compact = opts?.compact ?? false;
    return (
      <div
        className={
          compact
            ? "flex items-center gap-2 px-3 py-2"
            : "flex items-center gap-0.5 px-1"
        }
      >
        <span className="text-xs text-text-muted tabular-nums">
          {playbackSpeed.toFixed(1)}x
        </span>
        <input
          type="range"
          min="0.1"
          max="2"
          step="0.1"
          value={playbackSpeed}
          onChange={(e) => setPlaybackSpeed(parseFloat(e.target.value))}
          className={compact ? "flex-1 h-1 accent-brand" : "w-16 h-1 accent-brand"}
          title="Playback Speed"
          disabled={!hasJoints}
        />
      </div>
    );
  };

  return {
    byTab,
    tabs: ALL_TABS,
    renderSimulateExtras,
  };
}
