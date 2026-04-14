import { useState, useEffect, useCallback, useRef } from "react";
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
import { ToolbarButton, MoreDropdown } from "@/components/ui/toolbar";
import { TAB_COLORS, MOBILE_BREAKPOINT } from "@/components/ui/toolbar-constants";
import {
  useDocumentStore,
  useUiStore,
  useSketchStore,
  useEngineStore,
  useSimulationStore,
  exportStlBlob,
  exportGltfBlob,
  exportStepBlob,
  type ToolbarTab,
} from "@vcad/core";
import type { PrimitiveKind, BooleanType } from "@vcad/core";
import { isStitchEligible, getPcbNodeIds } from "@vcad/core";
import { downloadBlob } from "@/lib/download";
import { useNotificationStore } from "@/stores/notification-store";
import { useOutputStore, estimatePrice } from "@/stores/output-store";
import { cn } from "@/lib/utils";
import {
  InsertInstanceDialog,
  AddJointDialog,
  FilletChamferDialog,
  ShellDialog,
  PatternDialog,
  MirrorDialog,
  TextDialog,
  StitchDialog,
  NewPcbDialog,
} from "@/components/dialogs";
import { useOnboardingStore, type GuidedFlowStep } from "@/stores/onboarding-store";
import { useDrawingStore } from "@/stores/drawing-store";
import { useSlicerStore } from "@/stores/slicer-store";
import { useCamStore } from "@/stores/cam-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";
import { analytics } from "@/lib/analytics";

const PRIMITIVES: { kind: PrimitiveKind; icon: typeof Cube; label: string }[] = [
  { kind: "cube", icon: Cube, label: "Box" },
  { kind: "cylinder", icon: Cylinder, label: "Cylinder" },
  { kind: "sphere", icon: Globe, label: "Sphere" },
];

const BOOLEANS: {
  type: BooleanType;
  icon: typeof Unite;
  label: string;
  shortcut: string;
}[] = [
  { type: "union", icon: Unite, label: "Union", shortcut: "⌘⇧U" },
  { type: "difference", icon: Subtract, label: "Difference", shortcut: "⌘⇧D" },
  { type: "intersection", icon: Intersect, label: "Intersection", shortcut: "⌘⇧I" },
];


// All tabs in priority order (higher priority = shown first when space is limited)
const ALL_TABS: { id: ToolbarTab; label: string; icon: typeof Cube }[] = [
  { id: "create", label: "Create", icon: Cube },
  { id: "transform", label: "Transform", icon: ArrowsOutCardinal },
  { id: "combine", label: "Combine", icon: Unite },
  { id: "modify", label: "Modify", icon: Circle },
  { id: "assembly", label: "Assembly", icon: Package },
  { id: "simulate", label: "Simulate", icon: Play },
  { id: "build", label: "Export", icon: Export },
];

// Responsive breakpoints and widths
const TAB_WIDTH_DESKTOP = 95; // ~95px per tab on desktop
const TAB_WIDTH_MOBILE = 44; // Just icon on mobile
const CHAT_WIDTH = 70;
const MORE_WIDTH = 44;
const MIN_VISIBLE_TABS = 0; // Can collapse all to More on very small screens


export function ToolPalette() {
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const applyBoolean = useDocumentStore((s) => s.applyBoolean);
  const createPartDef = useDocumentStore((s) => s.createPartDef);
  const document = useDocumentStore((s) => s.document);

  const select = useUiStore((s) => s.select);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const transformMode = useUiStore((s) => s.transformMode);
  const setTransformMode = useUiStore((s) => s.setTransformMode);
  const toolbarExpanded = useUiStore((s) => s.toolbarExpanded);
  const toolbarTab = useUiStore((s) => s.toolbarTab);
  const setToolbarTab = useUiStore((s) => s.setToolbarTab);
  const enterSketchMode = useSketchStore((s) => s.enterSketchMode);
  const enterFaceSelectionMode = useSketchStore((s) => s.enterFaceSelectionMode);
  const sketchActive = useSketchStore((s) => s.active);
  const faceSelectionMode = useSketchStore((s) => s.faceSelectionMode);
  const parts = useDocumentStore((s) => s.parts);

  const [insertDialogOpen, setInsertDialogOpen] = useState(false);
  const [jointDialogOpen, setJointDialogOpen] = useState(false);
  const [filletDialogOpen, setFilletDialogOpen] = useState(false);
  const [chamferDialogOpen, setChamferDialogOpen] = useState(false);
  const [shellDialogOpen, setShellDialogOpen] = useState(false);
  const [patternDialogOpen, setPatternDialogOpen] = useState(false);
  const [mirrorDialogOpen, setMirrorDialogOpen] = useState(false);
  const [textDialogOpen, setTextDialogOpen] = useState(false);
  const [stitchDialogOpen, setStitchDialogOpen] = useState(false);
  const [pcbDialogOpen, setPcbDialogOpen] = useState(false);
  const [pcbFitWidth, setPcbFitWidth] = useState<number | undefined>();
  const [pcbFitHeight, setPcbFitHeight] = useState<number | undefined>();

  // Responsive toolbar - track how many tabs fit
  const [visibleTabCount, setVisibleTabCount] = useState(ALL_TABS.length);
  const toolbarRef = useRef<HTMLDivElement>(null);

  // Calculate visible tabs based on viewport width
  useEffect(() => {
    function calculateVisibleTabs() {
      const viewportWidth = window.innerWidth;
      const isMobile = viewportWidth < MOBILE_BREAKPOINT;
      const tabWidth = isMobile ? TAB_WIDTH_MOBILE : TAB_WIDTH_DESKTOP;

      // On mobile, be more aggressive - less padding, smaller tabs
      const padding = isMobile ? 24 : 40;
      const availableWidth = viewportWidth - padding;

      // Width needed for Chat + More buttons
      const fixedWidth = CHAT_WIDTH + MORE_WIDTH;
      // Remaining width for tabs
      const tabsWidth = availableWidth - fixedWidth;
      // How many tabs can fit
      const maxTabs = Math.max(MIN_VISIBLE_TABS, Math.floor(tabsWidth / tabWidth));
      setVisibleTabCount(Math.min(maxTabs, ALL_TABS.length));
    }

    calculateVisibleTabs();
    window.addEventListener("resize", calculateVisibleTabs);
    return () => window.removeEventListener("resize", calculateVisibleTabs);
  }, []);

  // Listen for custom events to open PCB dialog
  useEffect(() => {
    const onOpenPcb = () => {
      setPcbFitWidth(undefined);
      setPcbFitHeight(undefined);
      setPcbDialogOpen(true);
    };
    const onFitPcb = (e: Event) => {
      const detail = (e as CustomEvent<{ width: number; height: number }>).detail;
      setPcbFitWidth(detail.width);
      setPcbFitHeight(detail.height);
      setPcbDialogOpen(true);
    };
    window.addEventListener("vcad:open-pcb-dialog", onOpenPcb);
    window.addEventListener("vcad:fit-pcb-dialog", onFitPcb);
    return () => {
      window.removeEventListener("vcad:open-pcb-dialog", onOpenPcb);
      window.removeEventListener("vcad:fit-pcb-dialog", onFitPcb);
    };
  }, []);

  // Tabs that don't fit are stuffed into the More dropdown
  const overflowTabs = ALL_TABS.slice(visibleTabCount);

  // displayedTab is just toolbarTab (no more hover preview)
  const displayedTab = toolbarTab;

  // Drawing view state
  const viewMode = useDrawingStore((s) => s.viewMode);
  const setViewMode = useDrawingStore((s) => s.setViewMode);

  // Engine state
  const scene = useEngineStore((s) => s.scene);
  const hasSceneParts = Boolean(scene?.parts?.length);

  // Simulation state
  const simMode = useSimulationStore((s) => s.mode);
  const physicsAvailable = useSimulationStore((s) => s.physicsAvailable);
  const playbackSpeed = useSimulationStore((s) => s.playbackSpeed);
  const playSim = useSimulationStore((s) => s.play);
  const pauseSim = useSimulationStore((s) => s.pause);
  const stopSim = useSimulationStore((s) => s.stop);
  const stepSim = useSimulationStore((s) => s.step);
  const setPlaybackSpeed = useSimulationStore((s) => s.setPlaybackSpeed);

  // Guided flow state
  const guidedFlowActive = useOnboardingStore((s) => s.guidedFlowActive);
  const guidedFlowStep = useOnboardingStore((s) => s.guidedFlowStep);
  const advanceGuidedFlow = useOnboardingStore((s) => s.advanceGuidedFlow);

  // Output/Build state
  const openQuotePanel = useOutputStore((s) => s.openQuotePanel);
  const selectedMaterial = useOutputStore((s) => s.selectedMaterial);
  const estimatedPrice = estimatePrice(scene, selectedMaterial);
  const buildTooltip = !hasSceneParts
    ? "Build (add geometry first)"
    : estimatedPrice
      ? `Build (~$${estimatedPrice.toFixed(0)})`
      : "Build";

  // Helper to check if a button should pulse during guided flow
  function shouldPulse(
    forStep: GuidedFlowStep,
    extraCondition: boolean = true
  ): boolean {
    return guidedFlowActive && guidedFlowStep === forStep && extraCondition;
  }

  // Listen for insert-instance event from command palette
  useEffect(() => {
    function handleInsertInstance() {
      setInsertDialogOpen(true);
    }
    window.addEventListener("vcad:insert-instance", handleInsertInstance);
    return () =>
      window.removeEventListener("vcad:insert-instance", handleInsertInstance);
  }, []);

  const hasSelection = selectedPartIds.size > 0;
  const hasTwoSelected = selectedPartIds.size === 2;

  // Assembly mode detection
  const hasPartDefs = document.partDefs && Object.keys(document.partDefs).length > 0;
  const hasInstances = document.instances && document.instances.length > 0;
  const isAssemblyMode = hasPartDefs || hasInstances;
  const hasJoints = document.joints && document.joints.length > 0;

  // Check if we have one part selected (for create part def)
  const hasOnePartSelected =
    selectedPartIds.size === 1 && parts.some((p) => selectedPartIds.has(p.id));

  // Check if we have two instances selected (for add joint)
  const selectedInstanceIds = Array.from(selectedPartIds).filter((id) =>
    document.instances?.some((i) => i.id === id)
  );
  const hasTwoInstancesSelected = selectedInstanceIds.length === 2;

  // Check if an instance is selected (for assembly tab auto-switch)
  const hasInstanceSelected = Array.from(selectedPartIds).some((id) =>
    document.instances?.some((i) => i.id === id)
  );

  // Get the single selected part ID (for modify operations)
  const selectedPartId = hasOnePartSelected
    ? Array.from(selectedPartIds).find((id) => parts.some((p) => p.id === id))
    : null;

  // Check if selected part is eligible for stitch conversion
  const selectedPartStitchEligible = selectedPartId
    ? (() => {
        const p = parts.find((pp) => pp.id === selectedPartId);
        return p ? isStitchEligible(p) : false;
      })()
    : false;

  // Listen for modify operation events from command palette
  useEffect(() => {
    function handleFillet() {
      if (selectedPartId) setFilletDialogOpen(true);
    }
    function handleChamfer() {
      if (selectedPartId) setChamferDialogOpen(true);
    }
    function handleShell() {
      if (selectedPartId) setShellDialogOpen(true);
    }
    function handlePattern() {
      if (selectedPartId) setPatternDialogOpen(true);
    }
    function handleMirror() {
      if (selectedPartId) setMirrorDialogOpen(true);
    }
    function handleStitch() {
      if (selectedPartId) setStitchDialogOpen(true);
    }
    window.addEventListener("vcad:apply-fillet", handleFillet);
    window.addEventListener("vcad:apply-chamfer", handleChamfer);
    window.addEventListener("vcad:apply-shell", handleShell);
    window.addEventListener("vcad:apply-pattern", handlePattern);
    window.addEventListener("vcad:apply-mirror", handleMirror);
    window.addEventListener("vcad:apply-stitch", handleStitch);
    return () => {
      window.removeEventListener("vcad:apply-fillet", handleFillet);
      window.removeEventListener("vcad:apply-chamfer", handleChamfer);
      window.removeEventListener("vcad:apply-shell", handleShell);
      window.removeEventListener("vcad:apply-pattern", handlePattern);
      window.removeEventListener("vcad:apply-mirror", handleMirror);
      window.removeEventListener("vcad:apply-stitch", handleStitch);
    };
  }, [selectedPartId]);

  // Track manual tab clicks to temporarily disable auto-switch
  const manualOverrideRef = useRef(false);
  const manualOverrideTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleTabClick = useCallback((tab: ToolbarTab) => {
    // Set manual override for 2 seconds
    manualOverrideRef.current = true;
    if (manualOverrideTimeout.current) {
      clearTimeout(manualOverrideTimeout.current);
    }
    manualOverrideTimeout.current = setTimeout(() => {
      manualOverrideRef.current = false;
    }, 2000);
    setToolbarTab(tab);
  }, [setToolbarTab]);

  // Keyboard shortcuts: 1-8 to switch tabs
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Don't trigger if typing in an input
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      // Don't trigger with modifier keys
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      const tabIndex = parseInt(e.key) - 1;
      if (tabIndex >= 0 && tabIndex < ALL_TABS.length) {
        handleTabClick(ALL_TABS[tabIndex]!.id);
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleTabClick]);

  // Auto-switch tabs based on context
  const autoSwitchTab = useCallback(() => {
    // Don't auto-switch during guided flow or if user manually changed tabs recently
    if (guidedFlowActive || manualOverrideRef.current) return;

    // Switch to build tab when entering 2D mode
    if (viewMode === "2d") {
      setToolbarTab("build");
      return;
    }

    // Switch to assembly tab when instance is selected
    if (hasInstanceSelected && isAssemblyMode) {
      setToolbarTab("assembly");
      return;
    }

    // Switch to combine tab when exactly 2 parts selected
    if (hasTwoSelected) {
      setToolbarTab("combine");
      return;
    }

    // Switch to transform tab when 1+ parts selected
    if (hasSelection) {
      setToolbarTab("transform");
      return;
    }

    // Default to create when nothing selected
    if (!hasSelection && toolbarTab !== "modify" && toolbarTab !== "simulate" && toolbarTab !== "build") {
      setToolbarTab("create");
    }
  }, [
    guidedFlowActive,
    viewMode,
    hasInstanceSelected,
    isAssemblyMode,
    hasTwoSelected,
    hasSelection,
    toolbarTab,
    setToolbarTab,
  ]);

  // Run auto-switch on relevant state changes
  useEffect(() => {
    autoSwitchTab();
  }, [selectedPartIds.size, viewMode, hasInstanceSelected, autoSwitchTab]);

  function handleAddPrimitive(kind: PrimitiveKind) {
    const partId = addPrimitive(kind);
    select(partId);
    setTransformMode("translate");
    analytics.primitiveAdded(kind);

    // Advance guided flow if applicable
    if (guidedFlowActive) {
      if (guidedFlowStep === "add-cube" && kind === "cube") {
        advanceGuidedFlow();
      } else if (guidedFlowStep === "add-cylinder" && kind === "cylinder") {
        advanceGuidedFlow();
      }
    }
  }

  function handleBoolean(type: BooleanType) {
    if (!hasTwoSelected) return;
    const ids = Array.from(selectedPartIds);
    const newId = applyBoolean(type, ids[0]!, ids[1]!);
    if (newId) select(newId);
    analytics.booleanApplied(type);

    // Advance guided flow if subtracting during tutorial
    if (guidedFlowActive && guidedFlowStep === "subtract" && type === "difference") {
      advanceGuidedFlow();
    }
  }

  function handleCreatePartDef() {
    if (!hasOnePartSelected) return;
    const partId = Array.from(selectedPartIds)[0]!;
    const partDefId = createPartDef(partId);
    if (partDefId) {
      // Select the newly created instance
      const instance = document.instances?.find((i) => i.partDefId === partDefId);
      if (instance) {
        select(instance.id);
      }
    }
  }

  // Render tab content based on specified tab (or displayed tab if not specified)
  const renderTabContent = (tab?: ToolbarTab) => {
    const targetTab = tab ?? displayedTab;
    const color = TAB_COLORS[targetTab];

    switch (targetTab) {
      case "create":
        return (
          <>
            {PRIMITIVES.map(({ kind, icon: Icon, label }) => (
              <ToolbarButton
                key={kind}
                tooltip={`Add ${label}`}
                disabled={sketchActive}
                onClick={() => handleAddPrimitive(kind)}
                pulse={
                  (kind === "cube" && shouldPulse("add-cube")) ||
                  (kind === "cylinder" && shouldPulse("add-cylinder"))
                }
                expanded={toolbarExpanded}
                label={label}
                iconColor={color}
              >
                <Icon size={15} />
              </ToolbarButton>
            ))}
            <ToolbarButton
              tooltip="New Sketch (S)"
              active={faceSelectionMode}
              disabled={sketchActive}
              onClick={() => {
                analytics.sketchStarted();
                if (parts.length > 0) {
                  enterFaceSelectionMode();
                } else {
                  enterSketchMode("XY");
                }
              }}
              expanded={toolbarExpanded}
              label="Sketch"
              shortcut="S"
              iconColor={color}
            >
              <PencilSimple size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="Add Text"
              disabled={sketchActive}
              onClick={() => setTextDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Text"
              iconColor={color}
            >
              <TextT size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="Add PCB Board"
              disabled={sketchActive}
              onClick={() => {
                setPcbFitWidth(undefined);
                setPcbFitHeight(undefined);
                setPcbDialogOpen(true);
              }}
              expanded={toolbarExpanded}
              label="PCB"
              iconColor={color}
            >
              <Circuitry size={15} />
            </ToolbarButton>
          </>
        );

      case "transform":
        return (
          <>
            <ToolbarButton
              tooltip={!hasSelection ? "Move (select a part)" : "Move (M)"}
              active={hasSelection && transformMode === "translate"}
              disabled={!hasSelection || viewMode === "2d"}
              onClick={() => setTransformMode("translate")}
              expanded={toolbarExpanded}
              label="Move"
              shortcut="M"
              iconColor={color}
            >
              <ArrowsOutCardinal size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasSelection ? "Rotate (select a part)" : "Rotate (R)"}
              active={hasSelection && transformMode === "rotate"}
              disabled={!hasSelection || viewMode === "2d"}
              onClick={() => setTransformMode("rotate")}
              expanded={toolbarExpanded}
              label="Rotate"
              shortcut="R"
              iconColor={color}
            >
              <ArrowsClockwise size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasSelection ? "Scale (select a part)" : "Scale (S)"}
              active={hasSelection && transformMode === "scale"}
              disabled={!hasSelection || viewMode === "2d"}
              onClick={() => setTransformMode("scale")}
              expanded={toolbarExpanded}
              label="Scale"
              shortcut="S"
              iconColor={color}
            >
              <ArrowsOut size={15} />
            </ToolbarButton>
          </>
        );

      case "combine":
        return (
          <>
            {BOOLEANS.map(({ type, icon: Icon, label, shortcut }) => (
              <ToolbarButton
                key={type}
                tooltip={!hasTwoSelected ? `${label} (select 2 parts)` : `${label} (${shortcut})`}
                disabled={!hasTwoSelected}
                onClick={() => handleBoolean(type)}
                pulse={type === "difference" && shouldPulse("subtract")}
                expanded={toolbarExpanded}
                label={label}
                shortcut={shortcut}
                iconColor={color}
              >
                <Icon size={15} />
              </ToolbarButton>
            ))}
          </>
        );

      case "modify":
        return (
          <>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Fillet (select a part)" : "Fillet"}
              disabled={!hasOnePartSelected || sketchActive}
              onClick={() => setFilletDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Fillet"
              iconColor={color}
            >
              <Circle size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Chamfer (select a part)" : "Chamfer"}
              disabled={!hasOnePartSelected || sketchActive}
              onClick={() => setChamferDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Chamfer"
              iconColor={color}
            >
              <Octagon size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Shell (select a part)" : "Shell"}
              disabled={!hasOnePartSelected || sketchActive}
              onClick={() => setShellDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Shell"
              iconColor={color}
            >
              <CubeTransparent size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Pattern (select a part)" : "Pattern"}
              disabled={!hasOnePartSelected || sketchActive}
              onClick={() => setPatternDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Pattern"
              iconColor={color}
            >
              <DotsThree size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Mirror (select a part)" : "Mirror"}
              disabled={!hasOnePartSelected || sketchActive}
              onClick={() => setMirrorDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Mirror"
              iconColor={color}
            >
              <ArrowsHorizontal size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Stitch (select a part)" : !selectedPartStitchEligible ? "Stitch (requires text/extrude/revolve/sweep/loft)" : "Stitch"}
              disabled={!hasOnePartSelected || !selectedPartStitchEligible || sketchActive}
              onClick={() => setStitchDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Stitch"
              iconColor={color}
            >
              <Scissors size={15} />
            </ToolbarButton>
          </>
        );

      case "assembly":
        return (
          <>
            <ToolbarButton
              tooltip={!hasOnePartSelected ? "Create Part Definition (select a part)" : "Create Part Definition"}
              disabled={!hasOnePartSelected || sketchActive}
              onClick={handleCreatePartDef}
              expanded={toolbarExpanded}
              label="Create Part"
              iconColor={color}
            >
              <Package size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasPartDefs ? "Insert Instance (create a part def first)" : "Insert Instance"}
              disabled={!hasPartDefs || sketchActive}
              onClick={() => setInsertDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Insert"
              iconColor={color}
            >
              <PlusSquare size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasTwoInstancesSelected ? "Add Joint (select 2 instances)" : "Add Joint"}
              disabled={!hasTwoInstancesSelected || sketchActive}
              onClick={() => setJointDialogOpen(true)}
              expanded={toolbarExpanded}
              label="Joint"
              iconColor={color}
            >
              <LinkSimple size={15} />
            </ToolbarButton>
          </>
        );

      case "simulate":
        return (
          <>
            <ToolbarButton
              tooltip={
                !hasJoints
                  ? "Play (add joints to simulate)"
                  : simMode === "running"
                  ? "Pause Simulation"
                  : "Play Simulation"
              }
              active={simMode === "running"}
              disabled={!hasJoints || !physicsAvailable || sketchActive}
              onClick={() => {
                if (simMode === "running") {
                  pauseSim();
                } else {
                  analytics.physicsSimulationRun();
                  playSim();
                }
              }}
              expanded={toolbarExpanded}
              label={simMode === "running" ? "Pause" : "Play"}
              iconColor={color}
            >
              {simMode === "running" ? <Pause size={15} /> : <Play size={15} />}
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasJoints ? "Stop (add joints to simulate)" : "Stop Simulation"}
              disabled={!hasJoints || simMode === "off" || sketchActive}
              onClick={stopSim}
              expanded={toolbarExpanded}
              label="Stop"
              iconColor={color}
            >
              <Stop size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!hasJoints ? "Step (add joints to simulate)" : "Step Simulation"}
              disabled={!hasJoints || simMode === "running" || !physicsAvailable || sketchActive}
              onClick={stepSim}
              expanded={toolbarExpanded}
              label="Step"
              iconColor={color}
            >
              <FastForward size={15} />
            </ToolbarButton>
            <div className="flex items-center gap-0.5 px-1">
              <span className="text-xs text-text-muted">{playbackSpeed.toFixed(1)}x</span>
              <input
                type="range"
                min="0.1"
                max="2"
                step="0.1"
                value={playbackSpeed}
                onChange={(e) => setPlaybackSpeed(parseFloat(e.target.value))}
                className="w-16 h-1 accent-brand"
                title="Playback Speed"
                disabled={!hasJoints}
              />
            </div>
          </>
        );

      case "build":
        return (
          <>
            <ToolbarButton
              tooltip={buildTooltip}
              disabled={!hasSceneParts}
              onClick={() => {
                analytics.quotePanelOpened();
                window.dispatchEvent(new CustomEvent("vcad:hero-view"));
                openQuotePanel();
              }}
              expanded={toolbarExpanded}
              label="Build"
              iconColor="text-brand"
              className="bg-brand/10 hover:bg-brand/20 rounded"
              labelClassName="text-brand font-medium"
            >
              <Sparkle size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="3D View"
              active={viewMode === "3d"}
              onClick={() => setViewMode("3d")}
              expanded={toolbarExpanded}
              label="3D"
              iconColor={color}
            >
              <Cube3D size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="2D Drawing View"
              active={viewMode === "2d"}
              onClick={() => setViewMode("2d")}
              expanded={toolbarExpanded}
              label="2D"
              iconColor={color}
            >
              <Blueprint size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!scene?.parts?.length ? "Export STL (add geometry first)" : "Export STL"}
              disabled={!scene?.parts?.length}
              onClick={() => {
                if (scene) {
                  const blob = exportStlBlob(scene);
                  downloadBlob(blob, "model.stl");
                  analytics.documentExported("stl");
                  useNotificationStore.getState().addToast("Exported model.stl", "success");
                }
              }}
              expanded={toolbarExpanded}
              label="STL"
              iconColor={color}
            >
              <Download size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!scene?.parts?.length ? "Export GLB (add geometry first)" : "Export GLB"}
              disabled={!scene?.parts?.length}
              onClick={() => {
                if (scene) {
                  const blob = exportGltfBlob(scene);
                  downloadBlob(blob, "model.glb");
                  analytics.documentExported("glb");
                  useNotificationStore.getState().addToast("Exported model.glb", "success");
                }
              }}
              expanded={toolbarExpanded}
              label="GLB"
              iconColor={color}
            >
              <Download size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!scene?.parts?.length ? "Export STEP (add geometry first)" : "Export STEP"}
              disabled={!scene?.parts?.length}
              onClick={() => {
                if (scene) {
                  try {
                    const blob = exportStepBlob(scene);
                    downloadBlob(blob, "model.step");
                    analytics.documentExported("step");
                    useNotificationStore.getState().addToast("Exported model.step", "success");
                  } catch (e) {
                    useNotificationStore.getState().addToast(
                      e instanceof Error ? e.message : "STEP export failed",
                      "error"
                    );
                  }
                }
              }}
              expanded={toolbarExpanded}
              label="STEP"
              iconColor={color}
            >
              <Download size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip={!scene?.parts?.length ? "Print (add geometry first)" : "Open Print Settings"}
              disabled={!scene?.parts?.length || sketchActive}
              onClick={() => {
                analytics.printPanelOpened();
                useSlicerStore.getState().openPrintPanel();
              }}
              expanded={toolbarExpanded}
              label="Print"
              iconColor={color}
            >
              <Printer size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="Open CAM Panel"
              disabled={sketchActive}
              onClick={() => {
                useCamStore.getState().openCamPanel();
              }}
              expanded={toolbarExpanded}
              label="CAM"
              iconColor={color}
            >
              <Path size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="Open Electronics Workspace"
              disabled={sketchActive}
              onClick={() => {
                const doc = useDocumentStore.getState().document;
                const boardIds = getPcbNodeIds(doc);
                if (boardIds.length > 0) {
                  useElectronicsStore.getState().enter();
                } else {
                  setPcbFitWidth(undefined);
                  setPcbFitHeight(undefined);
                  setPcbDialogOpen(true);
                }
              }}
              expanded={toolbarExpanded}
              label="ECAD"
              iconColor={color}
            >
              <Circuitry size={15} />
            </ToolbarButton>
            <ToolbarButton
              tooltip="Open Embroidery Panel"
              disabled={sketchActive}
              onClick={() => {
                useEmbroideryStore.getState().openPanel();
              }}
              expanded={toolbarExpanded}
              label="Emb"
              iconColor={color}
            >
              <Scissors size={15} />
            </ToolbarButton>
          </>
        );

      default:
        return null;
    }
  };

  return (
    <>
      <InsertInstanceDialog
        open={insertDialogOpen}
        onOpenChange={setInsertDialogOpen}
      />
      <AddJointDialog open={jointDialogOpen} onOpenChange={setJointDialogOpen} />
      <TextDialog open={textDialogOpen} onOpenChange={setTextDialogOpen} />
      <NewPcbDialog
        open={pcbDialogOpen}
        onOpenChange={setPcbDialogOpen}
        initialWidth={pcbFitWidth}
        initialHeight={pcbFitHeight}
      />
      {selectedPartId && (
        <>
          <FilletChamferDialog
            open={filletDialogOpen}
            onOpenChange={setFilletDialogOpen}
            mode="fillet"
            partId={selectedPartId}
          />
          <FilletChamferDialog
            open={chamferDialogOpen}
            onOpenChange={setChamferDialogOpen}
            mode="chamfer"
            partId={selectedPartId}
          />
          <ShellDialog
            open={shellDialogOpen}
            onOpenChange={setShellDialogOpen}
            partId={selectedPartId}
          />
          <PatternDialog
            open={patternDialogOpen}
            onOpenChange={setPatternDialogOpen}
            partId={selectedPartId}
          />
          <MirrorDialog
            open={mirrorDialogOpen}
            onOpenChange={setMirrorDialogOpen}
            partId={selectedPartId}
          />
          <StitchDialog
            open={stitchDialogOpen}
            onOpenChange={setStitchDialogOpen}
            partId={selectedPartId}
          />
        </>
      )}
      {/* Tool palette — Borland C++ Builder Component Palette style:       */}
      {/*   Row 1: tab strip (click a tab to switch)                         */}
      {/*   Row 2: active tab's button row (rendered inline, not a popover)  */}
      <div
        ref={toolbarRef}
        className={cn(
          "tool-palette flex flex-col",
          "bg-surface",
        )}
      >
        {/* Row 1: tab strip */}
        <div className="flex h-7 items-stretch border-b border-border/40">
          {ALL_TABS.slice(0, visibleTabCount).map(({ id, label, icon: Icon }, index) => {
            const isActive = displayedTab === id;
            return (
              <button
                key={id}
                onClick={() => handleTabClick(id)}
                className={cn(
                  "flex items-center gap-1.5 px-3 text-[11px] font-medium border-b-2",
                  "transition-colors",
                  isActive
                    ? cn("border-brand text-text bg-hover/30")
                    : "border-transparent text-text-muted hover:text-text hover:bg-hover/20",
                )}
                title={`${index + 1}. ${label}`}
              >
                <Icon size={13} className={cn(isActive && TAB_COLORS[id])} />
                <span>{label}</span>
                <span className="ml-1 text-[9px] text-text-muted/60 font-mono hidden sm:inline">
                  {index + 1}
                </span>
              </button>
            );
          })}
          {overflowTabs.length > 0 && (
            <MoreDropdown
              tabs={overflowTabs}
              activeTab={toolbarTab}
              onSelect={handleTabClick}
              colors={TAB_COLORS}
            >
              {(tab) => renderTabContent(tab)}
            </MoreDropdown>
          )}
          <div className="flex-1" />
        </div>

        {/* Row 2: active tab content — rendered inline, not in a popover */}
        <div className="flex items-center gap-0.5 px-2 h-7">
          {renderTabContent(displayedTab)}
        </div>
      </div>
    </>
  );
}
