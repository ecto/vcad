import { useRef, useEffect, Suspense, lazy } from "react";
import * as THREE from "three";
import { Canvas, useThree } from "@react-three/fiber";
import { ViewportContent } from "./ViewportContent";
import { DrawingView } from "./DrawingView";
import { RayTracedViewportOverlay } from "./RayTracedViewport";
import {
  useUiStore,
  useDocumentStore,
  useEngineStore,
} from "@vcad/core";
import { useTheme } from "@/hooks/useTheme";
import { useDrawingStore } from "@/stores/drawing-store";
import { useElectronicsStore } from "@/stores/electronics-store";

const ElectronicsWorkspace = lazy(() =>
  import("./electronics").then((m) => ({ default: m.ElectronicsWorkspace })),
);

// Monokai Soda from tmTheme
const BG_DARK = "#222222";
const BG_LIGHT = "#f6f6ef";
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

      shiftHeld = e.shiftKey;
      startPoint = { x: e.clientX, y: e.clientY };
      isSelecting = true;

      overlayEl = document.createElement("div");
      overlayEl.className =
        "pointer-events-none absolute z-20 border-2 border-accent bg-accent/10";
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

  // Render Electronics Workspace
  if (electronicsActive) {
    return (
      <div
        className="absolute inset-0 overflow-hidden"
        style={{ backgroundColor: isDark ? "#0a0a0a" : "#ffffff" }}
      >
        <Suspense fallback={null}>
          <ElectronicsWorkspace />
        </Suspense>
      </div>
    );
  }

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

  // Render 3D Canvas
  return (
    <div ref={containerRef} className="absolute inset-0">
      <Canvas
        frameloop="demand"
        camera={{ position: [50, -50, 50], fov: 50, near: 0.1, far: 10000, up: [0, 0, 1] }}
        onPointerMissed={() => clearSelection()}
        onCreated={() => performance.mark("canvas-ready")}
        shadows
        gl={{
          antialias: true,
          logarithmicDepthBuffer: true,
          toneMapping: THREE.ACESFilmicToneMapping,
          toneMappingExposure: 1.0,
          preserveDrawingBuffer: true, // Required for pixel sampling (adaptive UI)
        }}
        style={{ background: isDark ? BG_DARK : BG_LIGHT }}
      >
        <ViewportContent />
        <BoxSelectHandler containerRef={containerRef} />
      </Canvas>
      {/* Ray-traced overlay - rendered outside Canvas to avoid R3F reconciler issues */}
      {renderMode === "raytrace" && raytraceAvailable && (
        <RayTracedViewportOverlay />
      )}
    </div>
  );
}
