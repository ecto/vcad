import {
  useState,
  useRef,
  useEffect,
  useCallback,
  useLayoutEffect,
  lazy,
  Suspense,
} from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { NotificationContainer } from "@/components/ui/notifications";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { AppShell } from "@/components/AppShell";
import { Header } from "@/components/Header";
import { StatusBar } from "@/components/StatusBar";
import { ToolPalette } from "@/components/ToolPalette";
import { ToolDialogs } from "@/components/ToolDialogs";
import { Viewport } from "@/components/Viewport";
import { FeatureTree } from "@/components/FeatureTree";
import { MobileShell } from "@/components/mobile/MobileShell";
import { useIsMobile } from "@/hooks/useIsMobile";

// Lazy-loaded components (behind user actions, modals, or conditional renders)
const PropertyPanel = lazy(() => import("@/components/PropertyPanel").then(m => ({ default: m.PropertyPanel })));
const SceneInspector = lazy(() => import("@/components/SceneInspector").then(m => ({ default: m.SceneInspector })));
const GuidedFlowOverlay = lazy(() => import("@/components/GuidedFlowOverlay").then(m => ({ default: m.GuidedFlowOverlay })));
const GhostPromptController = lazy(() => import("@/components/GhostPromptController").then(m => ({ default: m.GhostPromptController })));
const CelebrationOverlay = lazy(() => import("@/components/CelebrationOverlay").then(m => ({ default: m.CelebrationOverlay })));
const SignInDelight = lazy(() => import("@/components/SignInDelight").then(m => ({ default: m.SignInDelight })));
const AboutModal = lazy(() => import("@/components/AboutModal").then(m => ({ default: m.AboutModal })));
const ShareDialog = lazy(() => import("@/components/ShareDialog").then(m => ({ default: m.ShareDialog })));
const ForkPromptModal = lazy(() => import("@/components/ForkPromptModal").then(m => ({ default: m.ForkPromptModal })));
const ReadOnlyBanner = lazy(() => import("@/components/ReadOnlyBanner").then(m => ({ default: m.ReadOnlyBanner })));
const ProfilePage = lazy(() => import("@/components/ProfilePage").then(m => ({ default: m.ProfilePage })));
// UsernamePickerModal is lazy-loaded inside ShareDialog, not here.
const CommandPalette = lazy(() => import("@/components/CommandPalette").then(m => ({ default: m.CommandPalette })));
const SketchToolbar = lazy(() => import("@/components/SketchToolbar").then(m => ({ default: m.SketchToolbar })));
const SketchStatusPanel = lazy(() => import("@/components/SketchStatusPanel").then(m => ({ default: m.SketchStatusPanel })));
const DrawingToolbar = lazy(() => import("@/components/DrawingToolbar").then(m => ({ default: m.DrawingToolbar })));
const FaceSelectionOverlay = lazy(() => import("@/components/FaceSelectionOverlay").then(m => ({ default: m.FaceSelectionOverlay })));
const QuotePanel = lazy(() => import("@/components/QuotePanel").then(m => ({ default: m.QuotePanel })));
const LogViewer = lazy(() => import("@/components/LogViewer").then(m => ({ default: m.LogViewer })));
const PrintPanel = lazy(() => import("@/components/print").then(m => ({ default: m.PrintPanel })));
const DfmOverlay = lazy(() => import("@/components/print/DfmOverlay").then(m => ({ default: m.DfmOverlay })));
const CamPanel = lazy(() => import("@/components/cam").then(m => ({ default: m.CamPanel })));
const ChatSidebar = lazy(() => import("@/components/ChatSidebar").then(m => ({ default: m.ChatSidebar })));
const DocumentPicker = lazy(() => import("@/components/DocumentPicker").then(m => ({ default: m.DocumentPicker })));
const OfflineIndicator = lazy(() => import("@/components/OfflineIndicator").then(m => ({ default: m.OfflineIndicator })));
const UpdateNotification = lazy(() => import("@/components/UpdateNotification").then(m => ({ default: m.UpdateNotification })));
const WhatsNewPanel = lazy(() => import("@/components/WhatsNewPanel").then(m => ({ default: m.WhatsNewPanel })));
const ElectronicsToolbar = lazy(() => import("@/components/electronics/ElectronicsToolbar").then(m => ({ default: m.ElectronicsToolbar })));
const ElectronicsStatusPanel = lazy(() => import("@/components/electronics/ElectronicsStatusPanel").then(m => ({ default: m.ElectronicsStatusPanel })));
const EmbroideryPanel = lazy(() => import("@/components/embroidery").then(m => ({ default: m.EmbroideryPanel })));

import {
  useSketchStore,
  useEngineStore,
  useDocumentStore,
  useUiStore,
  useChatStore,
  isEmbroideryPatternPart,
  parseVcadFile,
  parseStl,
  logger,
  type VcadFile,
} from "@vcad/core";
import { useEngine } from "@/hooks/useEngine";
import { useKeyboardShortcuts } from "@/hooks/useKeyboardShortcuts";
import { useKeybindingDispatcher } from "@/hooks/useKeybindingDispatcher";
import { useAutoSave } from "@/hooks/useAutoSave";
import { useChatHandler } from "@/hooks/useChatHandler";
import { useUrlSync } from "@/hooks/useUrlSync";
import { saveDocument } from "@/lib/save-load";
import { bootstrap } from "@/lib/bootstrap";
import { useBootStore } from "@/stores/boot-store";
import { Splash } from "@/components/Splash";
import { ErrorScreen } from "@/components/ErrorScreen";
import { isTauri } from "@/lib/tauri";
import { getProfileRouteUsername } from "@/lib/url-document";
import {
  mergeMeshes,
} from "@vcad/engine";
import type { EmbroideryDesign } from "@vcad/ir";
import { useNotificationStore } from "@/stores/notification-store";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { useSlicerStore } from "@/stores/slicer-store";
import { useCamStore } from "@/stores/cam-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";

function useThemeSync() {
  const theme = useUiStore((s) => s.theme);
  useLayoutEffect(() => {
    const applyTheme = (prefersDark: boolean) => {
      const effectiveTheme =
        theme === "system" ? (prefersDark ? "dark" : "light") : theme;
      document.documentElement.classList.toggle(
        "light",
        effectiveTheme === "light",
      );
    };

    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    applyTheme(mq.matches);

    if (theme === "system") {
      const handler = (e: MediaQueryListEvent) => applyTheme(e.matches);
      mq.addEventListener("change", handler);
      return () => mq.removeEventListener("change", handler);
    }
  }, [theme]);
}

/** Left sidebar: tree by default, drills into inspector when something is selected. */
function FeatureTreeSlot({ sketchActive }: { sketchActive: boolean }) {
  const sidebarPane = useUiStore((s) => s.sidebarPane);
  const inspectorTarget = useUiStore((s) => s.inspectorTarget);
  if (sketchActive) return null;
  return (
    <div className="flex h-full w-full flex-col min-h-0">
      <div className="flex-1 min-h-0 overflow-hidden">
        {sidebarPane === "tree" ? (
          <FeatureTree />
        ) : inspectorTarget?.kind === "scene" ? (
          <Suspense fallback={null}>
            <SceneInspector />
          </Suspense>
        ) : (
          <Suspense fallback={null}>
            <PropertyPanel />
          </Suspense>
        )}
      </div>
    </div>
  );
}

export function App() {
  useEngine();
  useThemeSync();
  useAutoSave();
  useChatHandler();
  useUrlSync();

  const [aboutOpen, setAboutOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [documentPickerOpen, setDocumentPickerOpen] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const bootPhase = useBootStore((s) => s.phase);
  const bootError = useBootStore((s) => s.error);

  useEffect(() => {
    void bootstrap();
  }, []);

  const desktopMode = isTauri();
  const isMobile = useIsMobile();
  const engineReady = useEngineStore((s) => s.engineReady);
  const error = useEngineStore((s) => s.error);
  const sketchActive = useSketchStore((s) => s.active);
  const electronicsActive = useElectronicsStore((s) => s.active);

  const guidedFlowActive = useOnboardingStore((s) => s.guidedFlowActive);
  const guidedFlowStep = useOnboardingStore((s) => s.guidedFlowStep);
  const advanceGuidedFlow = useOnboardingStore((s) => s.advanceGuidedFlow);
  const incrementSessions = useOnboardingStore((s) => s.incrementSessions);
  const startGuidedFlow = useOnboardingStore((s) => s.startGuidedFlow);
  const parts = useDocumentStore((s) => s.parts);
  const selectMultiple = useUiStore((s) => s.selectMultiple);
  const printPanelOpen = useSlicerStore((s) => s.printPanelOpen);
  const camPanelOpen = useCamStore((s) => s.camPanelOpen);
  const embroideryPanelOpen = useEmbroideryStore((s) => s.panelOpen);
  const partIndex = useDocumentStore((s) => s.partIndex);
  const selIds = useUiStore((s) => s.selectedPartIds);
  const commandPaletteOpen = useUiStore((s) => s.commandPaletteOpen);
  const setCommandPaletteOpen = useUiStore((s) => s.setCommandPaletteOpen);
  const statusBarVisible = useUiStore((s) => s.statusBarVisible);
  const selPart = selIds.size === 1 ? partIndex.get(Array.from(selIds)[0]!) : undefined;
  const hasSelectedEmbroideryPart = selPart != null && isEmbroideryPatternPart(selPart);

  // Auto-drill the left sidebar into the inspector when something is selected,
  // and back to the tree when selection clears. Selecting a part also clears
  // the scene inspector target so the scene doesn't shadow the part.
  useEffect(() => {
    const { setSidebarPane, setInspectorTarget } = useUiStore.getState();
    if (selIds.size >= 1) {
      setInspectorTarget(null);
      setSidebarPane("inspector");
    } else if (useUiStore.getState().inspectorTarget == null) {
      setSidebarPane("tree");
    }
  }, [selIds]);

  const handleSave = useCallback(() => {
    const state = useDocumentStore.getState();
    saveDocument(state);
    useDocumentStore.getState().markSaved();
    useNotificationStore.getState().addToast("Document saved", "success");
  }, []);

  const handleOpen = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  // Global keybinding dispatcher — runs the shared Rust registry first.
  // useKeyboardShortcuts continues to handle bindings not yet in the
  // registry; it checks `e.defaultPrevented` and bails for any chord this
  // hook already dispatched.
  useKeybindingDispatcher({
    onAboutOpen: () => setAboutOpen(true),
    onSave: handleSave,
    onOpen: handleOpen,
  });
  useKeyboardShortcuts();

  const processFile = useCallback(async (file: File) => {
    const ext = file.name.split(".").pop()?.toLowerCase();

    // Handle STEP files
    if (ext === "step" || ext === "stp") {
      try {
        const engine = useEngineStore.getState().engine;
        if (!engine) {
          useNotificationStore.getState().addToast("Engine not ready", "error");
          return;
        }

        logger.info("step", "Starting import...");
        const buffer = await file.arrayBuffer();
        logger.info("step", `Buffer size: ${buffer.byteLength}`);

        logger.info("step", "Calling engine.importStep...");
        const rawMeshes = engine.importStep(buffer);
        logger.info("step", `Got meshes: ${rawMeshes.length}`);

        if (rawMeshes.length === 0) {
          useNotificationStore.getState().addToast("No geometry found in STEP file", "error");
          return;
        }

        // Log mesh sizes
        let totalTris = 0;
        rawMeshes.forEach((m, i) => {
          const tris = m.indices.length / 3;
          totalTris += tris;
          logger.debug("step", `Mesh ${i}: ${tris} triangles`);
        });
        logger.info("step", `Total: ${totalTris} triangles`);

        // Merge all meshes into one for better GPU performance (1 draw call instead of N)
        logger.info("step", "Merging meshes...");
        const mergedMesh = mergeMeshes(rawMeshes);
        logger.info("step", `Merged into 1 mesh with ${mergedMesh.indices.length / 3} triangles`);

        // Skip GPU normal computation for STEP imports — the wgpu compute
        // shader uses std::sync::mpsc which deadlocks in WASM's single-threaded
        // environment. Three.js computes vertex normals at render time instead.
        const finalPositions = mergedMesh.positions;
        const finalIndices = mergedMesh.indices;
        const finalNormals: Float32Array | undefined = undefined;

        // Add as a proper document part (not just a scene mesh)
        // This makes it selectable, deletable, and transformable
        useDocumentStore.getState().loadDocument({
          document: { version: "1", nodes: {}, roots: [], materials: {}, part_materials: {} },
          parts: [],
          nextNodeId: 1,
          nextPartNum: 1,
        });
        useDocumentStore.getState().addImportedMesh(
          finalPositions,
          finalIndices,
          finalNormals,
          file.name,
        );
        useUiStore.getState().clearSelection();

        useNotificationStore.getState().addToast(
          `Imported ${rawMeshes.length} solid${rawMeshes.length > 1 ? "s" : ""} from STEP (${totalTris.toLocaleString()} triangles)`,
          "success"
        );
      } catch (err) {
        console.error("Failed to import STEP:", err);
        useNotificationStore.getState().addToast("Failed to import STEP file", "error");
      }
      return;
    }

    // Handle KiCad PCB files
    if (ext === "kicad_pcb") {
      try {
        const text = await file.text();
        const { parseKicadPcb } = await import("@vcad/engine");
        const pcb = await parseKicadPcb(text);
        if (pcb) {
          useDocumentStore.getState().importPcb(pcb, file.name);
          useElectronicsStore.getState().enter();
          useNotificationStore.getState().addToast(`Imported ${file.name}`, "success");
        } else {
          useNotificationStore.getState().addToast("Failed to parse KiCad PCB file", "error");
        }
      } catch (err) {
        console.error("Failed to import KiCad PCB:", err);
        useNotificationStore.getState().addToast("Failed to import KiCad PCB file", "error");
      }
      return;
    }

    // Handle STL files
    if (ext === "stl") {
      try {
        const buffer = await file.arrayBuffer();
        const mesh = parseStl(buffer);
        const triangleCount = mesh.indices.length / 3;

        // Add as a proper document part (not just a scene mesh)
        // This makes it selectable, deletable, and exportable
        useDocumentStore.getState().loadDocument({
          document: { version: "1", nodes: {}, roots: [], materials: {}, part_materials: {} },
          parts: [],
          nextNodeId: 1,
          nextPartNum: 1,
        });
        useDocumentStore.getState().addImportedMesh(
          mesh.positions,
          mesh.indices,
          undefined, // STL doesn't have normals (computed from geometry)
          file.name,
        );
        useUiStore.getState().clearSelection();

        useNotificationStore.getState().addToast(
          `Imported STL with ${triangleCount.toLocaleString()} triangles`,
          "success"
        );
      } catch (err) {
        console.error("Failed to import STL:", err);
        useNotificationStore.getState().addToast("Failed to import STL file", "error");
      }
      return;
    }

    // Handle embroidery files
    if (ext === "pes" || ext === "dst") {
      try {
        const buffer = await file.arrayBuffer();
        const bytes = new Uint8Array(buffer);
        const wasm = await import("@vcad/kernel-wasm");
        const json = ext === "pes"
          ? wasm.readEmbroideryPes(bytes)
          : wasm.readEmbroideryDst(bytes);
        const result = JSON.parse(json);

        // Build EmbroideryDesign for the IR node
        const design: EmbroideryDesign = {
          threads: result.threads,
          stitch_groups: result.stitchPaths.map((sp: { threadIndex: number; points: [number, number][] }) => ({
            thread_index: sp.threadIndex,
            stitches: sp.points,
          })),
          hoop_width: result.stats.width,
          hoop_height: result.stats.height,
        };

        // Add to document as a proper node
        useDocumentStore.getState().addEmbroideryPattern(design, file.name);

        // Also populate embroidery store for the panel (stats, export, etc.)
        const store = useEmbroideryStore.getState();
        store.setFileName(file.name);
        store.setError(null);
        store.setSelectedFormat(ext as "pes" | "dst");
        store.setPattern({
          stitchCount: result.stats.stitchCount,
          colorCount: result.stats.colorCount,
          width: result.stats.width,
          height: result.stats.height,
          threads: result.threads,
          stitchPaths: result.stitchPaths,
        });
        store.setStats(result.stats);
        store.setPatternJson(result.patternJson);
        store.openPanel();
        useNotificationStore.getState().addToast(
          `Loaded ${file.name} (${result.stats.stitchCount.toLocaleString()} stitches)`,
          "success"
        );
      } catch (err) {
        console.error("Failed to load embroidery file:", err);
        useEmbroideryStore.getState().setError(String(err));
        useEmbroideryStore.getState().openPanel();
        useNotificationStore.getState().addToast("Failed to load embroidery file", "error");
      }
      return;
    }

    // Handle .vcad/.loon/.json files
    try {
      const text = await file.text();
      const engine = useEngineStore.getState().engine;
      const evalLoon = engine
        ? (source: string) => {
            const doc = engine.evalVcadSource(source);
            if (!doc) throw new Error("Loon evaluation not supported by this engine build");
            return JSON.stringify(doc);
          }
        : undefined;
      const vcadFile = parseVcadFile(text, evalLoon);
      useDocumentStore.getState().loadDocument(vcadFile);
      useUiStore.getState().clearSelection();
    } catch (err) {
      console.error("Failed to load file:", err);
      useNotificationStore.getState().addToast("Failed to load document", "error");
    }
  }, []);

  const handleFileChange = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      if (!file) return;
      await processFile(file);
      // Reset input so same file can be re-opened
      e.target.value = "";
    },
    [processFile],
  );

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragging(false);
  }, []);

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragging(false);
      const file = e.dataTransfer.files[0];
      if (file) await processFile(file);
    },
    [processFile],
  );

  const handleOpenDocuments = useCallback(() => {
    setDocumentPickerOpen(true);
  }, []);

  // Listen for save/open/documents/about/tutorial custom events from keyboard shortcuts
  useEffect(() => {
    const onSave = () => handleSave();
    const onOpen = () => handleOpen();
    const onDocuments = () => handleOpenDocuments();
    const onAbout = () => setAboutOpen(true);
    const onStartTutorial = () => startGuidedFlow();
    window.addEventListener("vcad:save", onSave);
    window.addEventListener("vcad:open", onOpen);
    window.addEventListener("vcad:documents", onDocuments);
    window.addEventListener("vcad:about", onAbout);
    window.addEventListener("vcad:start-tutorial", onStartTutorial);
    return () => {
      window.removeEventListener("vcad:save", onSave);
      window.removeEventListener("vcad:open", onOpen);
      window.removeEventListener("vcad:documents", onDocuments);
      window.removeEventListener("vcad:about", onAbout);
      window.removeEventListener("vcad:start-tutorial", onStartTutorial);
    };
  }, [handleSave, handleOpen, handleOpenDocuments, startGuidedFlow]);

  // Listen for load-example events from the menu
  useEffect(() => {
    const onLoadExample = (e: CustomEvent<{ file: VcadFile }>) => {
      try {
        useDocumentStore.getState().loadDocument(e.detail.file);
        useUiStore.getState().clearSelection();
      } catch (err) {
        console.error("Failed to load example:", err);
        useNotificationStore.getState().addToast("Failed to load example", "error");
      }
    };
    window.addEventListener(
      "vcad:load-example",
      onLoadExample as EventListener,
    );
    return () => {
      window.removeEventListener(
        "vcad:load-example",
        onLoadExample as EventListener,
      );
    };
  }, []);

  // Warn before closing with unsaved changes
  useEffect(() => {
    const handleBeforeUnload = (e: BeforeUnloadEvent) => {
      if (useDocumentStore.getState().isDirty) {
        e.preventDefault();
        e.returnValue = true;
      }
    };
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", handleBeforeUnload);
    };
  }, []);

  // Increment session counter on app load (for ghost prompt fade-out)
  useEffect(() => {
    incrementSessions();
  }, [incrementSessions]);

  // Track cylinder position for "position-cylinder" guided flow step
  const document = useDocumentStore((s) => s.document);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const featureTreeOpen = useUiStore((s) => s.featureTreeOpen);
  const chatOpen = useChatStore((s) => s.open);
  const cylinderInitialPos = useRef<{ x: number; y: number; z: number } | null>(null);

  useEffect(() => {
    if (!guidedFlowActive || guidedFlowStep !== "position-cylinder") {
      cylinderInitialPos.current = null;
      return;
    }

    // Find the cylinder part
    const cylinder = parts.find((p) => p.kind === "cylinder");
    if (!cylinder) return;

    // Get translation from document node
    const translateNode = document.nodes[String(cylinder.translateNodeId)];
    const offset =
      translateNode?.op.type === "Translate"
        ? translateNode.op.offset
        : { x: 0, y: 0, z: 0 };

    // Initialize baseline position
    if (cylinderInitialPos.current === null) {
      cylinderInitialPos.current = { ...offset };
    }

    // Check if cylinder has moved enough (>5mm in Y for "up")
    const deltaY = Math.abs(offset.y - cylinderInitialPos.current.y);
    if (deltaY > 5) {
      // Auto-select both parts for the subtract step
      // Order matters: first selected is the base, second is subtracted from it
      const cube = parts.find((p) => p.kind === "cube");
      if (cube && cylinder) {
        selectMultiple([cube.id, cylinder.id]);
      }
      advanceGuidedFlow();
    }
  }, [guidedFlowActive, guidedFlowStep, parts, document.nodes, advanceGuidedFlow, selectMultiple]);

  // Keep both parts selected during subtract step
  useEffect(() => {
    if (!guidedFlowActive || guidedFlowStep !== "subtract") return;

    const cube = parts.find((p) => p.kind === "cube");
    const cylinder = parts.find((p) => p.kind === "cylinder");
    if (!cube || !cylinder) return;

    // If not both selected, re-select them (cube first = base shape)
    const hasBoth = selectedPartIds.has(cube.id) && selectedPartIds.has(cylinder.id);
    if (!hasBoth) {
      selectMultiple([cube.id, cylinder.id]);
    }
  }, [guidedFlowActive, guidedFlowStep, parts, selectedPartIds, selectMultiple]);



  // Boot routing: splash until bootstrap reaches `ready`, error screen on
  // fatal boot failure. Post-boot engine errors fall through to the app.
  if (bootError) return <ErrorScreen message={bootError} />;
  if (error && !engineReady) return <ErrorScreen message={error} />;
  if (bootPhase !== "ready") return <Splash />;

  // /@username profile page — render a standalone page, not the editor.
  const profileUsername = getProfileRouteUsername();
  if (profileUsername) {
    return (
      <Suspense
        fallback={
          <div className="flex h-screen items-center justify-center bg-bg text-text-muted text-sm">
            Loading…
          </div>
        }
      >
        <ProfilePage username={profileUsername} />
      </Suspense>
    );
  }

  const viewportStack = (
    <>
      <Viewport />

      {/* Read-only share banner — fixed to the top of the viewport region */}
      <div className="absolute inset-x-0 top-0 z-30 pointer-events-none">
        <div className="pointer-events-auto">
          <Suspense fallback={null}>
            <ReadOnlyBanner />
          </Suspense>
        </div>
      </div>

      <Suspense fallback={null}>
        <SketchToolbar />
        <SketchStatusPanel />
        <DrawingToolbar />
        <FaceSelectionOverlay />
      </Suspense>

      {/* Electronics toolbar + status (self-gate via electronicsActive) */}
      {electronicsActive && (
        <Suspense fallback={null}>
          <ElectronicsToolbar />
          <ElectronicsStatusPanel />
        </Suspense>
      )}

      {/* Onboarding overlays */}
      <Suspense fallback={null}>
        <GuidedFlowOverlay />
        <GhostPromptController />
        <CelebrationOverlay />
        <SignInDelight />
      </Suspense>

      {/* Quote panel (slides in from right when Make It Real clicked) */}
      <Suspense fallback={null}>
        <QuotePanel />
      </Suspense>

      {/* Print panel (for 3D printing slicer settings) */}
      {printPanelOpen && <Suspense fallback={null}><PrintPanel /></Suspense>}
      {printPanelOpen && <Suspense fallback={null}><DfmOverlay /></Suspense>}

      {/* CAM panel (for CNC toolpath generation) */}
      {camPanelOpen && <Suspense fallback={null}><CamPanel /></Suspense>}

      {/* Embroidery panel — hide when an embroidery part is selected so PropertyPanel shows */}
      {embroideryPanelOpen && !hasSelectedEmbroideryPart && <Suspense fallback={null}><EmbroideryPanel /></Suspense>}
    </>
  );

  const dragOverlay = isDragging && (
    <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-brand/10 backdrop-blur-sm">
      <div className="rounded-lg border-2 border-dashed border-brand bg-bg/90 px-8 py-6 text-center">
        <div className="text-lg font-medium text-text">Drop file to import</div>
        <div className="mt-1 text-sm text-text-muted">.vcad, .loon, .stl, .step, .pes, .dst</div>
      </div>
    </div>
  );

  return (
    <ErrorBoundary>
      <TooltipProvider>
        <div
          className="contents"
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
          onDrop={handleDrop}
        >
          {isMobile ? (
            <MobileShell
              onAboutOpen={() => setAboutOpen(true)}
              onSave={handleSave}
              onOpen={handleOpen}
            >
              {viewportStack}
              {dragOverlay}
            </MobileShell>
          ) : (
          <AppShell
            header={!electronicsActive && (
              <Header
                onAboutOpen={() => setAboutOpen(true)}
                onSave={handleSave}
                onOpen={handleOpen}
                onShareOpen={() => setShareOpen(true)}
              >
                {!sketchActive && <ToolPalette />}
              </Header>
            )}
            leftSidebar={!electronicsActive && !sketchActive && featureTreeOpen && (
              <FeatureTreeSlot sketchActive={sketchActive} />
            )}
            rightSidebar={!electronicsActive && chatOpen && (
              <Suspense fallback={null}>
                <ChatSidebar />
              </Suspense>
            )}
            bottomDock={!electronicsActive && (
              <Suspense fallback={null}>
                <LogViewer />
              </Suspense>
            )}
            footer={!electronicsActive && statusBarVisible && <StatusBar />}
          >
          {viewportStack}
          {dragOverlay}
        </AppShell>
          )}

        {/* Offline indicator (browser only — Tauri doesn't use PWA) */}
        {!desktopMode && (
          <Suspense fallback={null}>
            <OfflineIndicator />
            <UpdateNotification />
          </Suspense>
        )}

        {/* Modals */}
        <Suspense fallback={null}>
          <AboutModal open={aboutOpen} onOpenChange={setAboutOpen} />
          <ShareDialog open={shareOpen} onOpenChange={setShareOpen} />
          <ForkPromptModal />
          <DocumentPicker
            open={documentPickerOpen}
            onOpenChange={setDocumentPickerOpen}
          />
          <CommandPalette
            open={commandPaletteOpen}
            onOpenChange={setCommandPaletteOpen}
            onAboutOpen={() => setAboutOpen(true)}
          />
        </Suspense>
        <input
          ref={fileInputRef}
          type="file"
          accept=".vcad,.loon,.json,.step,.stp,.stl,.pes,.dst,.kicad_pcb"
          className="hidden"
          onChange={handleFileChange}
        />
        <NotificationContainer />
        <ToolDialogs />
        <Suspense fallback={null}>
          <WhatsNewPanel />
        </Suspense>
      </div>
      </TooltipProvider>
    </ErrorBoundary>
  );
}
