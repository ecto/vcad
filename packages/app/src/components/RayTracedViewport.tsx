import { useRef, useEffect, useCallback } from "react";
import { useThree, useFrame } from "@react-three/fiber";
import { useEngineStore, useDocumentStore, useUiStore, logger } from "@vcad/core";
import { getRayTracer } from "@vcad/engine";
import { getMaterialByKey } from "@/data/materials";
import type { PerspectiveCamera } from "three";

// Store for syncing camera state from R3F to external overlay
type CameraState = {
  position: [number, number, number];
  target: [number, number, number];
  fov: number;
  width: number;
  height: number;
};

let cameraStateCallback: ((state: CameraState) => void) | null = null;

function setCameraStateCallback(cb: ((state: CameraState) => void) | null) {
  cameraStateCallback = cb;
}

/**
 * Internal component that syncs R3F camera state and triggers renders.
 * Must be placed inside the R3F Canvas.
 */
export function RayTracedViewportSync() {
  const scene = useEngineStore((s) => s.scene);
  const document = useDocumentStore((s) => s.document);
  const { camera, size, controls } = useThree();
  const rayTracer = getRayTracer();

  // Track if we've uploaded the scene
  const uploadedRef = useRef(false);

  // Track last camera state for dirty checking
  const lastCameraRef = useRef({ x: 0, y: 0, z: 0, tx: 0, ty: 0, tz: 0 });

  // Upload scene when it changes
  useEffect(() => {
    if (!rayTracer || !scene?.parts?.length) {
      uploadedRef.current = false;
      return;
    }

    // Try to upload first part with a solid
    let uploaded = false;
    let materialKey: string | undefined;
    for (const p of scene.parts) {
      const solid = (p as { solid?: unknown }).solid;
      if (!solid) continue;

      try {
        rayTracer.uploadSolid(solid);
        materialKey = p.material;
        uploaded = true;
        break;
      } catch {
        // Try next solid
      }
    }

    // Apply material — document overrides take precedence, fall back to preset library.
    if (uploaded && materialKey) {
      const docMat = document.materials[materialKey];
      const preset = docMat ? null : getMaterialByKey(materialKey);
      const mat = docMat ?? preset;
      if (mat) {
        try {
          rayTracer.setMaterial(
            mat.color[0],
            mat.color[1],
            mat.color[2],
            mat.metallic,
            mat.roughness
          );
        } catch (e) {
          logger.debug("gpu", `Failed to set material: ${e}`);
        }
      }
    }

    uploadedRef.current = uploaded;
  }, [scene, rayTracer, document.materials]);

  // Sync camera state on every frame
  useFrame(() => {
    if (!rayTracer || !uploadedRef.current || !cameraStateCallback) return;

    const cam = camera as PerspectiveCamera;

    // Get orbit target from controls if available
    const orbitControls = controls as { target?: { x: number; y: number; z: number } } | null;
    const target = orbitControls?.target ?? { x: 0, y: 0, z: 0 };

    // Check if camera changed (dirty check)
    const last = lastCameraRef.current;
    const EPSILON = 0.001;
    const cameraMoved =
      Math.abs(cam.position.x - last.x) > EPSILON ||
      Math.abs(cam.position.y - last.y) > EPSILON ||
      Math.abs(cam.position.z - last.z) > EPSILON ||
      Math.abs(target.x - last.tx) > EPSILON ||
      Math.abs(target.y - last.ty) > EPSILON ||
      Math.abs(target.z - last.tz) > EPSILON;

    if (cameraMoved) {
      lastCameraRef.current = {
        x: cam.position.x,
        y: cam.position.y,
        z: cam.position.z,
        tx: target.x,
        ty: target.y,
        tz: target.z,
      };

      cameraStateCallback({
        position: [cam.position.x, cam.position.y, cam.position.z],
        target: [target.x, target.y, target.z],
        fov: cam.fov,
        width: size.width,
        height: size.height,
      });
    }
  });

  return null;
}

/**
 * Ray-traced viewport overlay that renders BRep geometry directly
 * without tessellation artifacts.
 *
 * This component renders an HTML canvas that overlays the Three.js scene,
 * providing pixel-perfect silhouettes for CAD geometry.
 *
 * Must be placed OUTSIDE the R3F Canvas (as a sibling).
 */
// Map debug mode names to shader mode numbers
const DEBUG_MODE_MAP: Record<string, number> = {
  "off": 0,
  "normals": 1,
  "face-id": 2,
  "lighting": 3,
  "orientation": 4,
};

export function RayTracedViewportOverlay() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const raytraceQuality = useUiStore((s) => s.raytraceQuality);
  const raytraceDebugMode = useUiStore((s) => s.raytraceDebugMode);
  const raytraceEdgesEnabled = useUiStore((s) => s.raytraceEdgesEnabled);
  const raytraceEdgeDepthThreshold = useUiStore((s) => s.raytraceEdgeDepthThreshold);
  const raytraceEdgeNormalThreshold = useUiStore((s) => s.raytraceEdgeNormalThreshold);
  const rayTracer = getRayTracer();

  // Track last debug mode to detect changes
  const lastDebugModeRef = useRef<string>("off");

  // Track last edge settings to detect changes
  const lastEdgeSettingsRef = useRef({
    enabled: true,
    depth: 0.1,
    normal: 30.0,
  });

  // Track pending async render to avoid overlapping calls
  const renderInProgressRef = useRef(false);
  const needsRenderRef = useRef(false);
  const lastCameraStateRef = useRef<CameraState | null>(null);

  // Quality multiplier for render resolution
  const qualityScale =
    raytraceQuality === "draft" ? 0.5 : raytraceQuality === "high" ? 2 : 1;

  // Helper to draw pixels to canvas
  const drawPixels = useCallback(
    (
      ctx: CanvasRenderingContext2D,
      pixels: Uint8Array,
      renderWidth: number,
      renderHeight: number,
      canvasWidth: number,
      canvasHeight: number
    ) => {
      const imageData = new ImageData(
        new Uint8ClampedArray(pixels),
        renderWidth,
        renderHeight
      );

      // Scale to canvas if needed
      if (qualityScale !== 1 || renderWidth !== canvasWidth) {
        ctx.imageSmoothingEnabled = true;
        ctx.imageSmoothingQuality = "high";
        const tempCanvas = document.createElement("canvas");
        tempCanvas.width = renderWidth;
        tempCanvas.height = renderHeight;
        const tempCtx = tempCanvas.getContext("2d");
        if (tempCtx) {
          tempCtx.putImageData(imageData, 0, 0);
          ctx.drawImage(tempCanvas, 0, 0, canvasWidth, canvasHeight);
        }
      } else {
        ctx.putImageData(imageData, 0, 0);
      }
    },
    [qualityScale]
  );

  // Internal render function
  const doRenderInternal = useCallback(
    (state: CameraState) => {
      if (!rayTracer || !canvasRef.current || renderInProgressRef.current) return;

      const canvas = canvasRef.current;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      // Update canvas size if needed
      if (canvas.width !== state.width || canvas.height !== state.height) {
        canvas.width = state.width;
        canvas.height = state.height;
      }

      const currentFrame = (rayTracer?.getFrameIndex?.() as number) ?? 0;

      // Quality-based resolution limits
      // During interaction (first few frames), use lower resolution
      // As samples accumulate, allow higher resolution
      const MAX_PIXELS_BY_QUALITY = {
        draft: 640 * 480,       // 307k pixels
        standard: 1280 * 720,   // 921k pixels
        high: 1920 * 1080,      // 2M pixels
      } as const;

      // During first frame (camera moving), use draft quality for responsiveness
      const isInteracting = currentFrame <= 1;
      const effectiveQuality = isInteracting ? "draft" : raytraceQuality;
      const maxPixels = MAX_PIXELS_BY_QUALITY[effectiveQuality as keyof typeof MAX_PIXELS_BY_QUALITY] ?? MAX_PIXELS_BY_QUALITY.standard;

      let renderWidth = Math.floor(state.width * qualityScale);
      let renderHeight = Math.floor(state.height * qualityScale);
      const totalPixels = renderWidth * renderHeight;
      if (totalPixels > maxPixels) {
        const scale = Math.sqrt(maxPixels / totalPixels);
        renderWidth = Math.floor(renderWidth * scale);
        renderHeight = Math.floor(renderHeight * scale);
      }

      // Start async render
      renderInProgressRef.current = true;

      // Store dimensions for the async callback
      const w = renderWidth;
      const h = renderHeight;
      const cw = state.width;
      const ch = state.height;

      (
        rayTracer.render(
          state.position,
          state.target,
          [0, 0, 1], // up vector (Z-up)
          renderWidth,
          renderHeight,
          (state.fov * Math.PI) / 180
        ) as Promise<Uint8Array>
      )
        .then((pixels: Uint8Array) => {
          const drawCanvas = canvasRef.current;
          if (drawCanvas) {
            const drawCtx = drawCanvas.getContext("2d");
            if (drawCtx) {
              drawPixels(drawCtx, pixels, w, h, cw, ch);
            }
          }
          renderInProgressRef.current = false;

          // If camera moved while rendering, render again
          if (needsRenderRef.current && lastCameraStateRef.current) {
            needsRenderRef.current = false;
            doRenderInternal(lastCameraStateRef.current);
          }
        })
        .catch((e: Error) => {
          logger.debug("gpu", `Render failed: ${e}`);
          renderInProgressRef.current = false;
        });
    },
    [rayTracer, qualityScale, drawPixels, raytraceQuality]
  );

  // Public render function
  const doRender = useCallback(
    (state: CameraState) => {
      doRenderInternal(state);
    },
    [doRenderInternal]
  );

  // Register callback for camera updates
  useEffect(() => {
    setCameraStateCallback((state) => {
      lastCameraStateRef.current = state;
      if (!renderInProgressRef.current) {
        doRender(state);
      } else {
        needsRenderRef.current = true;
      }
    });

    return () => {
      setCameraStateCallback(null);
    };
  }, [doRender]);

  // Re-render when quality changes
  useEffect(() => {
    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [raytraceQuality, doRender]);

  // Apply debug mode changes to raytracer
  useEffect(() => {
    console.log(`[DEBUG] Debug mode effect running: mode=${raytraceDebugMode}, lastMode=${lastDebugModeRef.current}, hasRayTracer=${!!rayTracer}`);

    if (!rayTracer) {
      console.log("[DEBUG] No rayTracer available");
      return;
    }
    if (raytraceDebugMode === lastDebugModeRef.current) {
      console.log("[DEBUG] Debug mode unchanged, skipping");
      return;
    }

    lastDebugModeRef.current = raytraceDebugMode;
    const modeNumber = DEBUG_MODE_MAP[raytraceDebugMode] ?? 0;

    console.log(`[DEBUG] Setting debug mode: ${raytraceDebugMode} -> ${modeNumber}`);

    // Check if method exists
    const rt = rayTracer as { setDebugMode?: (mode: number) => void };
    const hasMethod = typeof rt.setDebugMode === "function";
    console.log(`[DEBUG] setDebugMode method exists: ${hasMethod}`);

    if (!hasMethod) {
      console.log("[DEBUG] WARNING: setDebugMode not available - WASM may need rebuild");
      return;
    }

    rt.setDebugMode!(modeNumber);
    console.log(`[DEBUG] Called setDebugMode(${modeNumber})`);

    // Re-render to see the change
    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [raytraceDebugMode, rayTracer, doRender]);

  // Apply edge detection settings changes
  useEffect(() => {
    if (!rayTracer) return;

    const last = lastEdgeSettingsRef.current;
    if (
      raytraceEdgesEnabled === last.enabled &&
      raytraceEdgeDepthThreshold === last.depth &&
      raytraceEdgeNormalThreshold === last.normal
    ) {
      return;
    }

    lastEdgeSettingsRef.current = {
      enabled: raytraceEdgesEnabled,
      depth: raytraceEdgeDepthThreshold,
      normal: raytraceEdgeNormalThreshold,
    };

    const rt = rayTracer as { setEdgeDetection?: (enabled: boolean, depth: number, normal: number) => void };
    const hasMethod = typeof rt.setEdgeDetection === "function";
    if (!hasMethod) {
      logger.debug("gpu", "setEdgeDetection not available - WASM may need rebuild");
      return;
    }

    rt.setEdgeDetection!(
      raytraceEdgesEnabled,
      raytraceEdgeDepthThreshold,
      raytraceEdgeNormalThreshold
    );

    // Re-render to see the change
    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [raytraceEdgesEnabled, raytraceEdgeDepthThreshold, raytraceEdgeNormalThreshold, rayTracer, doRender]);

  if (!rayTracer) {
    return null;
  }

  return (
    <canvas
      ref={canvasRef}
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        width: "100%",
        height: "100%",
        pointerEvents: "none",
      }}
    />
  );
}

/**
 * @deprecated Use RayTracedViewportSync inside Canvas and RayTracedViewportOverlay outside.
 */
export function RayTracedViewport() {
  // This component is now split into two parts for proper rendering.
  // Return null to avoid errors - the new components should be used instead.
  return null;
}
