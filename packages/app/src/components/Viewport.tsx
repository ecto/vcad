import { useRef, useEffect, Suspense, lazy } from "react";
import * as THREE from "three";
import { Canvas, useThree } from "@react-three/fiber";
import { ViewportContent } from "./ViewportContent";
import { DrawingView } from "./DrawingView";
import { RayTracedViewportOverlay } from "./RayTracedViewport";
import { FollowModeToggle } from "./FollowModeToggle";
import {
  useUiStore,
  useDocumentStore,
  useEngineStore,
} from "@vcad/core";
import { useTheme } from "@/hooks/useTheme";
import { useDrawingStore } from "@/stores/drawing-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useElectronicsSync } from "@/hooks/useElectronicsSync";
import {
  viewportPointerDown,
  viewportPointerMove,
  viewportPointerUp,
  viewportWasDrag,
} from "@/lib/viewport-drag";

const SchematicOverlayPanel = lazy(() =>
  import("./electronics/SchematicOverlayPanel").then((m) => ({
    default: m.SchematicOverlayPanel,
  })),
);
const ElectronicsPropertyPanel = lazy(() =>
  import("./electronics/ElectronicsPropertyPanel").then((m) => ({
    default: m.ElectronicsPropertyPanel,
  })),
);
const LengthTunePanel = lazy(() =>
  import("./electronics/LengthTunePanel").then((m) => ({
    default: m.LengthTunePanel,
  })),
);
const PcbGettingStarted = lazy(() =>
  import("./electronics/PcbGettingStarted").then((m) => ({
    default: m.PcbGettingStarted,
  })),
);

// Monokai Soda from tmTheme
export const BG_DARK = "#222222";
export const BG_LIGHT = "#e8eaec";
const MIN_DRAG_THRESHOLD = 5;

function BoxSelectHandler({
  containerRef,
}: {
  containerRef: React.RefObject<HTMLDivElement | null>;
}) {
  const { camera, size } = useThree();
  const scene = useEngineStore((s) => s.scene);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Capture container in closure so TS knows it's not null
    const containerEl = container;

    let isSelecting = false;
    let startPoint = { x: 0, y: 0 };
    let shiftHeld = false;
    let overlayEl: HTMLDivElement | null = null;

    function handlePointerDown(e: PointerEvent) {
      if (e.button !== 0) return;
      // Box-select is a mouse-only gesture. On touch, one-finger drag
      // rotates the camera via OrbitControls, so we must not also draw
      // a selection rectangle.
      if (e.pointerType !== "mouse") return;

      shiftHeld = e.shiftKey;
      startPoint = { x: e.clientX, y: e.clientY };
      isSelecting = true;

      overlayEl = document.createElement("div");
      overlayEl.className =
        "pointer-events-none absolute z-20 border-2 border-brand bg-brand/10";
      overlayEl.style.left = "0";
      overlayEl.style.top = "0";
      overlayEl.style.width = "0";
      overlayEl.style.height = "0";
      overlayEl.style.display = "none";
      containerEl.appendChild(overlayEl);
    }

    function handlePointerMove(e: PointerEvent) {
      if (!isSelecting || !overlayEl) return;

      const rect = containerEl.getBoundingClientRect();
      const left = Math.min(startPoint.x, e.clientX) - rect.left;
      const top = Math.min(startPoint.y, e.clientY) - rect.top;
      const width = Math.abs(e.clientX - startPoint.x);
      const height = Math.abs(e.clientY - startPoint.y);

      if (width >= MIN_DRAG_THRESHOLD || height >= MIN_DRAG_THRESHOLD) {
        overlayEl.style.display = "block";
        overlayEl.style.left = `${left}px`;
        overlayEl.style.top = `${top}px`;
        overlayEl.style.width = `${width}px`;
        overlayEl.style.height = `${height}px`;
      }
    }

    function handlePointerUp(e: PointerEvent) {
      if (!isSelecting) return;
      isSelecting = false;

      if (overlayEl) {
        overlayEl.remove();
        overlayEl = null;
      }

      const dx = Math.abs(e.clientX - startPoint.x);
      const dy = Math.abs(e.clientY - startPoint.y);

      if (dx < MIN_DRAG_THRESHOLD && dy < MIN_DRAG_THRESHOLD) {
        return;
      }

      const currentScene = useEngineStore.getState().scene;
      if (!currentScene) return;

      const partsState = useDocumentStore.getState().parts;
      const selectedState = useUiStore.getState().selectedPartIds;

      const rect = containerEl.getBoundingClientRect();
      const minX = Math.min(startPoint.x, e.clientX) - rect.left;
      const maxX = Math.max(startPoint.x, e.clientX) - rect.left;
      const minY = Math.min(startPoint.y, e.clientY) - rect.top;
      const maxY = Math.max(startPoint.y, e.clientY) - rect.top;

      const selectedIds: string[] = [];

      partsState.forEach((part, index) => {
        const evalPart = currentScene.parts[index];
        if (!evalPart) return;

        const mesh = evalPart.mesh;
        if (!mesh.positions.length) return;

        const box = new THREE.Box3();
        const pos = new THREE.Vector3();
        for (let i = 0; i < mesh.positions.length; i += 3) {
          pos.set(
            mesh.positions[i]!,
            mesh.positions[i + 1]!,
            mesh.positions[i + 2]!,
          );
          box.expandByPoint(pos);
        }

        const corners = [
          new THREE.Vector3(box.min.x, box.min.y, box.min.z),
          new THREE.Vector3(box.max.x, box.min.y, box.min.z),
          new THREE.Vector3(box.min.x, box.max.y, box.min.z),
          new THREE.Vector3(box.max.x, box.max.y, box.min.z),
          new THREE.Vector3(box.min.x, box.min.y, box.max.z),
          new THREE.Vector3(box.max.x, box.min.y, box.max.z),
          new THREE.Vector3(box.min.x, box.max.y, box.max.z),
          new THREE.Vector3(box.max.x, box.max.y, box.max.z),
        ];

        let partMinX = Infinity,
          partMaxX = -Infinity,
          partMinY = Infinity,
          partMaxY = -Infinity;

        for (const corner of corners) {
          corner.project(camera);
          const screenX = ((corner.x + 1) / 2) * size.width;
          const screenY = ((1 - corner.y) / 2) * size.height;
          partMinX = Math.min(partMinX, screenX);
          partMaxX = Math.max(partMaxX, screenX);
          partMinY = Math.min(partMinY, screenY);
          partMaxY = Math.max(partMaxY, screenY);
        }

        if (
          partMaxX >= minX &&
          partMinX <= maxX &&
          partMaxY >= minY &&
          partMinY <= maxY
        ) {
          selectedIds.push(part.id);
        }
      });

      if (shiftHeld) {
        const newSelection = new Set([...selectedState, ...selectedIds]);
        useUiStore.getState().selectMultiple(Array.from(newSelection));
      } else {
        useUiStore.getState().selectMultiple(selectedIds);
      }
    }

    containerEl.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);

    return () => {
      containerEl.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      if (overlayEl) overlayEl.remove();
    };
  }, [camera, size, containerRef, scene]);

  return null;
}

export function Viewport() {
  const containerRef = useRef<HTMLDivElement>(null);
  const clearSelection = useUiStore((s) => s.clearSelection);
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const { isDark } = useTheme();
  const viewMode = useDrawingStore((s) => s.viewMode);
  const electronicsActive = useElectronicsStore((s) => s.active);

  // Run electronics sync when in electronics mode
  useElectronicsSync();

  // Track drag distance on the viewport so click handlers can ignore the
  // click that follows a camera rotation / pan gesture. Without this, on
  // touch a one-finger rotate also selects whichever mesh the finger was
  // over at pointerup.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const onDown = (e: PointerEvent) => viewportPointerDown(e.clientX, e.clientY);
    const onMove = (e: PointerEvent) => viewportPointerMove(e.clientX, e.clientY);
    const onUp = () => viewportPointerUp();
    const onContextMenu = (e: MouseEvent) => {
      // Suppress the browser context menu over the canvas (long-press on
      // touch, right-click on desktop) so it doesn't interrupt gestures.
      e.preventDefault();
    };
    el.addEventListener("pointerdown", onDown);
    // Listen on window so we still see the up even if the pointer leaves
    // the container mid-gesture.
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    el.addEventListener("contextmenu", onContextMenu);
    return () => {
      el.removeEventListener("pointerdown", onDown);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      el.removeEventListener("contextmenu", onContextMenu);
    };
  }, []);

  // Render 2D Drawing View
  if (viewMode === "2d") {
    return (
      <div
        className="absolute inset-0 overflow-hidden"
        style={{ backgroundColor: isDark ? "#0a0a0a" : "#ffffff" }}
      >
        <DrawingView />
      </div>
    );
  }

  // Render 3D Canvas (shared for CAD and PCB modes)
  const canvasBg = electronicsActive
    ? (isDark ? "#0a0a0a" : "#f5f5f5")
    : (isDark ? BG_DARK : BG_LIGHT);

  return (
    <div
      ref={containerRef}
      data-viewport-root
      className="absolute inset-0"
      style={{ touchAction: "none", userSelect: "none", WebkitUserSelect: "none" }}
    >
      <Canvas
        frameloop="demand"
        camera={{ position: [50, 50, 50], fov: 50, near: 0.1, far: 10000 }}
        onPointerMissed={() => {
          // Ignore the click that follows a drag/rotate gesture.
          if (viewportWasDrag()) return;
          if (!electronicsActive) clearSelection();
        }}
        onCreated={() => performance.mark("canvas-ready")}
        shadows
        gl={{
          antialias: true,
          logarithmicDepthBuffer: true,
          toneMapping: THREE.ACESFilmicToneMapping,
          toneMappingExposure: 1.0,
          preserveDrawingBuffer: true,
        }}
        style={{ background: canvasBg }}
      >
        <ViewportContent mode={electronicsActive ? "pcb" : "3d"} />
        {!electronicsActive && <BoxSelectHandler containerRef={containerRef} />}
      </Canvas>

      {/* Electronics overlay panels (outside Canvas, on top) */}
      {electronicsActive && (
        <>
          <Suspense fallback={null}>
            <SchematicOverlayPanel />
          </Suspense>
          <Suspense fallback={null}>
            <ElectronicsPropertyPanel />
          </Suspense>
          <Suspense fallback={null}>
            <LengthTunePanel />
          </Suspense>
          <Suspense fallback={null}>
            <PcbGettingStarted />
          </Suspense>
        </>
      )}

      {/* Ray-traced overlay - rendered outside Canvas to avoid R3F reconciler issues */}
      {!electronicsActive && renderMode === "raytrace" && raytraceAvailable && (
        <RayTracedViewportOverlay />
      )}

      {/* Presence bar: who else is in this document, plus follow/lock controls.
          Pointer-events none on the wrapper so the canvas still receives orbit
          drags in the gaps; the toggle itself re-enables them. */}
      {!electronicsActive && (
        <div className="pointer-events-none absolute top-3 right-3 z-10">
          <FollowModeToggle />
        </div>
      )}
    </div>
  );
}
