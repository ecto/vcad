import { useRef, useEffect, useCallback } from "react";
import { useThree, useFrame } from "@react-three/fiber";
import { useEngineStore, useDocumentStore, useUiStore, logger } from "@vcad/core";
import { getRayTracer, solveForwardKinematics } from "@vcad/engine";
import { getMaterialByKey } from "@/data/materials";
import { useTheme } from "@/hooks/useTheme";
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

/**
 * Latest camera state captured by `RayTracedViewportSync`'s `useFrame`.
 * Lives outside React because the overlay (which fires renders) lives outside
 * the R3F Canvas — we can't share state through props or context cleanly.
 */
let latestCameraState: CameraState | null = null;

/**
 * R3F's `invalidate` from inside the Canvas. The overlay (outside the Canvas)
 * uses this to ask R3F to tick a frame — needed because `useFrame` only runs
 * on demand, and the overlay's mount happens *after* Sync's first frame.
 */
let invalidateFromSync: (() => void) | null = null;

/**
 * The mounted overlay canvas. Exposed so the video recorder can call
 * `captureStream` on it directly — the overlay sits on top of the R3F WebGL
 * canvas and is the only surface that shows raytraced pixels.
 */
let overlayCanvasEl: HTMLCanvasElement | null = null;

/**
 * Forces a single raytrace render at the standard tier, bypassing the
 * draft→standard→high refinement scheduler. The recorder calls this once per
 * captured video frame so animation is visible in the recording (the normal
 * scheduler only fires renders on camera change).
 */
let forceRenderFn: (() => void) | null = null;

/** Returns the live raytrace overlay canvas, or null when not mounted. */
export function getRayTracedOverlayCanvas(): HTMLCanvasElement | null {
  return overlayCanvasEl;
}

/**
 * Trigger a single immediate raytrace render at the standard tier.
 * No-op if the overlay isn't mounted. Safe to call from outside React.
 */
export function triggerRaytraceRender(): void {
  forceRenderFn?.();
}

function setCameraStateCallback(cb: ((state: CameraState) => void) | null) {
  cameraStateCallback = cb;
}

/**
 * Internal component that syncs R3F camera state and triggers renders.
 * Must be placed inside the R3F Canvas.
 */
export function RayTracedViewportSync() {
  const scene = useEngineStore((s) => s.scene);
  const engine = useEngineStore((s) => s.engine);
  const document = useDocumentStore((s) => s.document);
  const { camera, size, controls, invalidate } = useThree();
  const rayTracer = getRayTracer();

  // Track if we've uploaded the scene
  const uploadedRef = useRef(false);

  // Track last camera state for dirty checking
  const lastCameraRef = useRef({ x: 0, y: 0, z: 0, tx: 0, ty: 0, tz: 0 });

  // Force one render on initial upload, even if the camera is stationary —
  // the dirty-check useFrame below otherwise short-circuits forever when the
  // user toggles into raytrace without orbiting.
  const needsInitialRenderRef = useRef(false);

  // Upload scene when it changes. The engine-store scene comes from the
  // worker, so it has no `solid` handles (handles can't cross threads).
  // Re-evaluate on the main thread to materialize them for upload.
  useEffect(() => {
    if (!rayTracer || !engine || !document) {
      uploadedRef.current = false;
      return;
    }

    let solidScene;
    try {
      solidScene = engine.evaluateWithSolids(document);
    } catch (e) {
      logger.warn("gpu", `Ray-trace eval failed: ${e}`);
      uploadedRef.current = false;
      return;
    }

    const hasParts = (solidScene.parts?.length ?? 0) > 0;
    const hasInstances = (solidScene.instances?.length ?? 0) > 0;
    if (!hasParts && !hasInstances) {
      uploadedRef.current = false;
      return;
    }

    // Clear any previously-uploaded scene then merge each part's solid in.
    // The WASM accumulates surfaces/faces/BVH across uploads under a unified
    // root, so multi-part documents render in a single ray-trace pass.
    //
    // Setters are safe to call while render() is awaiting — RayTracer keeps
    // its mutable state behind a RefCell, so each call briefly borrows-and-
    // drops without conflicting with an in-flight render.
    type SolidHandle = {
      scale: (x: number, y: number, z: number) => SolidHandle;
      rotate: (x_deg: number, y_deg: number, z_deg: number) => SolidHandle;
      translate: (x: number, y: number, z: number) => SolidHandle;
    };
    const rt = rayTracer as {
      clearScene?: () => void;
      uploadSolid: (s: unknown) => void;
      uploadSolidWithMaterial?: (
        s: unknown,
        r: number,
        g: number,
        b: number,
        m: number,
        ro: number,
      ) => void;
      setMaterial: (r: number, g: number, b: number, m: number, ro: number) => void;
      setMaterialFromDef?: (json: string | null, tint: number[] | null) => void;
    };
    rt.clearScene?.();

    // Resolve a material key to (color, metallic, roughness) — document
    // overrides first, preset library second.
    const resolveMaterial = (key: string | undefined) => {
      if (!key) return null;
      const docMat = document.materials[key];
      const mat = docMat ?? getMaterialByKey(key);
      return mat
        ? {
            color: mat.color as [number, number, number],
            metallic: mat.metallic,
            roughness: mat.roughness,
          }
        : null;
    };

    // Upload with a per-solid material when the WASM build supports it
    // (GpuScene::merge keeps material slots distinct per upload).
    const uploadWithMaterial = (solid: unknown, key: string | undefined) => {
      const mat = resolveMaterial(key);
      if (mat && typeof rt.uploadSolidWithMaterial === "function") {
        rt.uploadSolidWithMaterial(
          solid,
          mat.color[0],
          mat.color[1],
          mat.color[2],
          mat.metallic,
          mat.roughness,
        );
      } else {
        rt.uploadSolid(solid);
      }
    };

    let uploaded = false;
    let firstMaterialKey: string | undefined;

    // Top-level parts (non-assembly docs, or root-level parts in mixed docs).
    for (const p of solidScene.parts ?? []) {
      const solid = (p as { solid?: unknown }).solid;
      if (!solid) continue;

      try {
        uploadWithMaterial(solid, p.material);
      } catch (e) {
        logger.debug("gpu", `uploadSolid failed: ${e}`);
      }
      if (firstMaterialKey === undefined) firstMaterialKey = p.material;
      uploaded = true;
    }

    // Assembly instances. Each instance references a partDef by id; we
    // transform the partDef's solid by the instance's world transform
    // (translate ∘ rotate ∘ scale, matching the Three.js TRS path used by
    // the tessellated renderer in `SceneMesh.tsx`) and upload one merged
    // copy per instance. Re-using the partDef solid is safe — translate/
    // rotate/scale return *new* solids without mutating the original.
    if (hasInstances && solidScene.partDefs?.length) {
      const partDefSolids = new Map<string, SolidHandle>();
      for (const pd of solidScene.partDefs) {
        const s = (pd as { solid?: unknown }).solid as SolidHandle | undefined;
        if (s) partDefSolids.set(pd.id, s);
      }

      // Jointed instances carry no useful `transform` of their own — the
      // joint places them (same FK contract as the tessellated renderer).
      // Without this, assembly parts pile up unposed at the origin.
      const fkTransforms = solveForwardKinematics(document);

      for (const inst of solidScene.instances ?? []) {
        const baseSolid = partDefSolids.get(inst.partDefId);
        if (!baseSolid) continue;
        const instId =
          (inst as { id?: string; instanceId?: string }).id ??
          (inst as { instanceId?: string }).instanceId;
        const tf = (instId ? fkTransforms.get(instId) : undefined) ?? inst.transform;
        const placed = tf
          ? baseSolid
              .scale(tf.scale.x, tf.scale.y, tf.scale.z)
              .rotate(tf.rotation.x, tf.rotation.y, tf.rotation.z)
              .translate(tf.translation.x, tf.translation.y, tf.translation.z)
          : baseSolid;
        try {
          uploadWithMaterial(placed, inst.material);
        } catch (e) {
          logger.debug("gpu", `uploadSolid (instance) failed: ${e}`);
        }
        if (firstMaterialKey === undefined) firstMaterialKey = inst.material;
        uploaded = true;
      }
    }

    // Apply material — document overrides take precedence, fall back to preset library.
    // For now we apply one material to the whole merged scene; per-part materials
    // would need a setMaterialAt(idx, ...) on the WASM side.
    if (uploaded && firstMaterialKey && typeof rt.uploadSolidWithMaterial !== "function") {
      const docMat = document.materials[firstMaterialKey];
      const preset = docMat ? null : getMaterialByKey(firstMaterialKey);
      const mat = docMat ?? preset;
      if (mat) {
        try {
          // Prefer the full definition: setMaterial only carries colour,
          // metallic and roughness, so clearcoat, IOR and anisotropy would be
          // dropped and a brushed or lacquered part would shade differently
          // here than under `vcad-render --photoreal`. setMaterialFromDef runs
          // the same Rust derivation both renderers share.
          // Fall back rather than losing the material outright: if the shape
          // ever drifts from the Rust MaterialDef the deserialize throws, and
          // dropping to setMaterial costs only the extended fields.
          let applied = false;
          if (typeof rt.setMaterialFromDef === "function") {
            try {
              rt.setMaterialFromDef(JSON.stringify(mat), null);
              applied = true;
            } catch (e) {
              logger.debug("gpu", `setMaterialFromDef rejected the definition, falling back: ${e}`);
            }
          }
          if (!applied) {
            rt.setMaterial(mat.color[0], mat.color[1], mat.color[2], mat.metallic, mat.roughness);
          }
        } catch (e) {
          logger.debug("gpu", `Failed to set material: ${e}`);
        }
      }
    }

    uploadedRef.current = uploaded;
    if (uploaded) {
      needsInitialRenderRef.current = true;

      // Capture camera state directly so the overlay (which mounts after this
      // effect) can pull it on register. With `frameloop="demand"`, useFrame
      // below isn't guaranteed to tick before the user orbits — the initial
      // render would otherwise wait on an interaction that defeats the
      // purpose of "I just toggled raytrace on, where's the picture?"
      const cam = camera as PerspectiveCamera;
      const orbitControls = controls as { target?: { x: number; y: number; z: number } } | null;
      const target = orbitControls?.target ?? { x: 0, y: 0, z: 0 };
      const state: CameraState = {
        position: [cam.position.x, cam.position.y, cam.position.z],
        target: [target.x, target.y, target.z],
        fov: cam.fov,
        width: size.width,
        height: size.height,
      };
      latestCameraState = state;
      lastCameraRef.current = {
        x: cam.position.x,
        y: cam.position.y,
        z: cam.position.z,
        tx: target.x,
        ty: target.y,
        tz: target.z,
      };

      if (cameraStateCallback) {
        needsInitialRenderRef.current = false;
        cameraStateCallback(state);
      }
      // Otherwise: overlay's mount effect will pull `latestCameraState` and
      // fire the first render itself. Also tick R3F so useFrame can take
      // over for subsequent camera moves.
      invalidate();
    }
  }, [scene, rayTracer, engine, document, camera, controls, size, invalidate]);

  // Expose R3F's invalidate so the overlay (mounted outside the Canvas) can
  // tell R3F to tick a frame after it registers the camera-state callback.
  useEffect(() => {
    invalidateFromSync = invalidate;
    return () => {
      invalidateFromSync = null;
    };
  }, [invalidate]);

  // Sync camera state on every frame
  useFrame(() => {
    if (!rayTracer || !uploadedRef.current) return;

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

    if (!cameraMoved && !needsInitialRenderRef.current) return;

    lastCameraRef.current = {
      x: cam.position.x,
      y: cam.position.y,
      z: cam.position.z,
      tx: target.x,
      ty: target.y,
      tz: target.z,
    };

    const state: CameraState = {
      position: [cam.position.x, cam.position.y, cam.position.z],
      target: [target.x, target.y, target.z],
      fov: cam.fov,
      width: size.width,
      height: size.height,
    };
    latestCameraState = state;

    if (cameraStateCallback) {
      needsInitialRenderRef.current = false;
      cameraStateCallback(state);
    }
    // If the callback isn't registered yet (overlay mounts after Sync's first
    // frame), keep needsInitialRenderRef true; the overlay will pull
    // latestCameraState directly when it mounts.
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

// Per-tier render resolution settings used by the refinement scheduler.
// `scale` multiplies canvas size before clamping to `maxPixels`; the rendered
// image is then upscaled to canvas size with bilinear filtering.
const TIER_CONFIG: Record<
  "draft" | "standard" | "high",
  { scale: number; maxPixels: number }
> = {
  draft: { scale: 0.5, maxPixels: 640 * 480 },        // ~307k px — instant
  standard: { scale: 1.0, maxPixels: 1280 * 720 },    // 720p cap — interactive
  high: { scale: 2.0, maxPixels: 1920 * 1080 },       // 2× DPI — final
};

// Ceiling on path-tracer depth. Matches `PathTraceOptions::default().max_depth`
// on the CPU side so a converged viewport image agrees with
// `vcad-render --photoreal`. The kernel escalates toward this with accumulation
// (2 bounces on the draft frame, then 4, then the full 6), so raising this does
// not make the first frame slower.
const PATH_TRACE_MAX_DEPTH = 6;

export function RayTracedViewportOverlay() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const raytraceQuality = useUiStore((s) => s.raytraceQuality);
  const raytraceDebugMode = useUiStore((s) => s.raytraceDebugMode);
  const raytraceEdgesEnabled = useUiStore((s) => s.raytraceEdgesEnabled);
  const raytraceEdgeDepthThreshold = useUiStore((s) => s.raytraceEdgeDepthThreshold);
  const raytraceEdgeNormalThreshold = useUiStore((s) => s.raytraceEdgeNormalThreshold);
  const raytraceEdgeSilhouetteEnabled = useUiStore((s) => s.raytraceEdgeSilhouetteEnabled);
  const raytraceEdgeCreaseEnabled = useUiStore((s) => s.raytraceEdgeCreaseEnabled);
  const raytraceEdgeBoundaryEnabled = useUiStore((s) => s.raytraceEdgeBoundaryEnabled);
  const raytraceEdgeSilhouetteWidth = useUiStore((s) => s.raytraceEdgeSilhouetteWidth);
  const raytraceEdgeCreaseWidth = useUiStore((s) => s.raytraceEdgeCreaseWidth);
  const raytraceEdgeBoundaryWidth = useUiStore((s) => s.raytraceEdgeBoundaryWidth);
  const raytraceEdgeSoftness = useUiStore((s) => s.raytraceEdgeSoftness);
  const rayTracer = getRayTracer();
  const { isDark } = useTheme();

  // Track last debug mode to detect changes
  const lastDebugModeRef = useRef<string>("off");

  // Track last edge settings to detect changes
  const lastEdgeSettingsRef = useRef({
    enabled: true,
    depth: 0.1,
    normal: 30.0,
    silhouette: true,
    crease: true,
    boundary: true,
    silhouetteWidth: 1.0,
    creaseWidth: 0.75,
    boundaryWidth: 1.25,
    softness: 1.5,
  });

  // Track last path-tracer settings to detect changes.
  const lastPathTraceSettingsRef = useRef({ stylize: true });

  // Track pending async render to avoid overlapping calls
  const renderInProgressRef = useRef(false);
  const lastCameraStateRef = useRef<CameraState | null>(null);

  // Refinement state machine. On every camera/scene change we drop to draft
  // for instant feedback, then progressively refine to standard, then high
  // (capped at the user's chosen ceiling), accumulating extra frames at the
  // top tier so stochastic terms (soft shadows, AO, env-spec jitter) average
  // into a noise-free image.
  const refinementTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const userQualityRef = useRef(raytraceQuality);
  useEffect(() => {
    userQualityRef.current = raytraceQuality;
  }, [raytraceQuality]);

  // Helper to draw pixels to canvas. Always upscales to canvas size when the
  // render resolution differs (i.e. for any tier other than 1:1 standard).
  const drawPixels = useCallback(
    (
      ctx: CanvasRenderingContext2D,
      pixels: Uint8Array,
      renderWidth: number,
      renderHeight: number,
      canvasWidth: number,
      canvasHeight: number,
    ) => {
      const imageData = new ImageData(
        new Uint8ClampedArray(pixels),
        renderWidth,
        renderHeight,
      );

      if (renderWidth !== canvasWidth || renderHeight !== canvasHeight) {
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
    [],
  );

  // Render at an explicit tier. Resolution and pixel cap come from the tier,
  // not from the user's quality setting — the refinement scheduler is what
  // decides which tier to render at.
  const doRenderAtTier = useCallback(
    (state: CameraState, tier: "draft" | "standard" | "high") => {
      if (!rayTracer || !canvasRef.current || renderInProgressRef.current) return;

      const canvas = canvasRef.current;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      if (canvas.width !== state.width || canvas.height !== state.height) {
        canvas.width = state.width;
        canvas.height = state.height;
      }

      const cfg = TIER_CONFIG[tier];
      let renderWidth = Math.floor(state.width * cfg.scale);
      let renderHeight = Math.floor(state.height * cfg.scale);
      const totalPixels = renderWidth * renderHeight;
      if (totalPixels > cfg.maxPixels) {
        const s = Math.sqrt(cfg.maxPixels / totalPixels);
        renderWidth = Math.floor(renderWidth * s);
        renderHeight = Math.floor(renderHeight * s);
      }

      renderInProgressRef.current = true;

      const w = renderWidth;
      const h = renderHeight;
      const cw = state.width;
      const ch = state.height;

      // R3F camera is Y-up display space; raytracer renders Z-up kernel
      // space. Transform position + target before handing off so the views
      // align (and DOM overlays project to the right pixels).
      //   display(x, y, z)  →  kernel(x, -z, y)
      const [px, py, pz] = state.position;
      const [tx, ty, tz] = state.target;
      const kernelPos: [number, number, number] = [px, -pz, py];
      const kernelTarget: [number, number, number] = [tx, -tz, ty];

      // RayTracer.render takes `&self` and snapshots its mutable state under
      // a brief borrow before awaiting GPU work — concurrent setter calls
      // (theme/debug/edges/upload) coexist safely.
      (rayTracer.render(
        kernelPos,
        kernelTarget,
        [0, 0, 1],
        renderWidth,
        renderHeight,
        (state.fov * Math.PI) / 180,
      ) as Promise<Uint8Array>)
        .then((pixels: Uint8Array) => {
          const drawCanvas = canvasRef.current;
          if (drawCanvas) {
            const drawCtx = drawCanvas.getContext("2d");
            if (drawCtx) {
              drawPixels(drawCtx, pixels, w, h, cw, ch);
            }
          }
          renderInProgressRef.current = false;
          // Clear any previous error now that a render landed cleanly.
          const errState = useUiStore.getState();
          if (errState.raytraceError !== null) {
            errState.setRaytraceError(null);
          }
        })
        .catch((e: Error) => {
          const msg = e?.message ?? String(e);
          logger.warn("gpu", `Render failed: ${msg}`);
          // Surface to the user via the footer chip. Silent failures are
          // what made the WGSL-shadowing bug in PR #185 so hard to find.
          useUiStore.getState().setRaytraceError(msg);
          renderInProgressRef.current = false;
        });
    },
    [rayTracer, drawPixels],
  );

  // Cancel any in-flight refinement timer (called on camera change).
  const cancelRefinement = useCallback(() => {
    if (refinementTimerRef.current) {
      clearTimeout(refinementTimerRef.current);
      refinementTimerRef.current = null;
    }
  }, []);

  // Walk the refinement plan: draft → standard → high, capped at user
  // ceiling. Each tier change resets the accumulation buffer (new resolution
  // would invalidate it anyway). At the top tier we keep ticking extra
  // frames so soft shadows / AO / env-spec jitter average into clean output.
  const runRefinement = useCallback(
    (state: CameraState) => {
      cancelRefinement();
      if (!rayTracer) return;

      const order = ["draft", "standard", "high"] as const;
      const maxIdx = order.indexOf(userQualityRef.current);

      const plan: Array<{
        tier: "draft" | "standard" | "high";
        frames: number;
        gap: number;
      }> = [{ tier: "draft", frames: 1, gap: 0 }];
      if (maxIdx >= 1) plan.push({ tier: "standard", frames: 4, gap: 80 });
      if (maxIdx >= 2) plan.push({ tier: "high", frames: 24, gap: 80 });

      // Reset accumulation when the chain starts (new camera state).
      const rt = rayTracer as { resetAccumulation?: () => void };
      rt.resetAccumulation?.();

      let stepIdx = 0;
      let frameIdx = 0;

      const tick = () => {
        if (stepIdx >= plan.length) return;
        const step = plan[stepIdx]!;

        if (frameIdx >= step.frames) {
          // Tier complete — advance to the next tier (if any).
          stepIdx++;
          frameIdx = 0;
          if (stepIdx >= plan.length) return;
          rt.resetAccumulation?.();
          refinementTimerRef.current = setTimeout(tick, plan[stepIdx]!.gap);
          return;
        }

        doRenderAtTier(state, step.tier);
        frameIdx++;
        // Inter-frame delay scales with tier — draft is one-shot, standard
        // is medium gap (UI stays responsive), high is slowest tick (lets
        // each render fully complete before the next is queued).
        const gap = step.tier === "high" ? 60 : 30;
        refinementTimerRef.current = setTimeout(tick, gap);
      };

      tick();
    },
    [cancelRefinement, doRenderAtTier, rayTracer],
  );

  // Backwards-compat alias used by the camera-state callback below.
  const doRender = runRefinement;

  // Expose the overlay canvas so the video recorder can captureStream it.
  // (Module-level pointer because the recorder hook lives inside the R3F
  // Canvas tree and the overlay does not — sharing through context would
  // require lifting state up past the Canvas boundary.)
  useEffect(() => {
    overlayCanvasEl = canvasRef.current;
    return () => {
      if (overlayCanvasEl === canvasRef.current) overlayCanvasEl = null;
    };
  }, []);

  // Expose a one-shot render hook for the recorder. Each call renders the
  // current camera state at the standard tier (no refinement chain) so a
  // single recorder tick produces a single fresh frame — animation is
  // visible without waiting for the draft→standard→high pyramid.
  useEffect(() => {
    forceRenderFn = () => {
      const state = lastCameraStateRef.current;
      if (state) doRenderAtTier(state, "standard");
    };
    return () => {
      forceRenderFn = null;
    };
  }, [doRenderAtTier]);

  // Register callback for camera updates. Each new camera state cancels
  // the in-progress refinement and starts a new one — the user gets an
  // instant draft on every gesture, then clean refinement once the camera
  // settles.
  useEffect(() => {
    const cb = (state: CameraState) => {
      lastCameraStateRef.current = state;
      doRender(state);
    };
    setCameraStateCallback(cb);

    // If Sync already pushed a state before we registered, render it now —
    // otherwise the toggle-and-stand-still case never gets a first frame.
    if (latestCameraState) {
      cb(latestCameraState);
    } else {
      // No state yet; ask R3F to tick so Sync's useFrame produces one.
      invalidateFromSync?.();
    }

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

  // Push theme into the WASM ray tracer when it flips, then kick a fresh
  // render. `setTheme` resets accumulation internally so the new background
  // palette appears cleanly without ghosting.
  useEffect(() => {
    if (!rayTracer) return;
    const rt = rayTracer as { setTheme?: (n: number) => void };
    rt.setTheme?.(isDark ? 0 : 1);
    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [isDark, rayTracer, doRender]);

  // Apply debug mode changes to raytracer
  useEffect(() => {
    if (!rayTracer) return;
    if (raytraceDebugMode === lastDebugModeRef.current) return;

    lastDebugModeRef.current = raytraceDebugMode;
    const modeNumber = DEBUG_MODE_MAP[raytraceDebugMode] ?? 0;

    const rt = rayTracer as { setDebugMode?: (mode: number) => void };
    if (typeof rt.setDebugMode !== "function") {
      logger.debug("gpu", "setDebugMode not available - WASM may need rebuild");
      return;
    }

    rt.setDebugMode(modeNumber);

    // Re-render to see the change
    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [raytraceDebugMode, rayTracer, doRender]);

  // Apply edge detection settings changes (master switch + thresholds)
  useEffect(() => {
    if (!rayTracer) return;

    const last = lastEdgeSettingsRef.current;
    if (
      raytraceEdgesEnabled === last.enabled &&
      raytraceEdgeDepthThreshold === last.depth &&
      raytraceEdgeNormalThreshold === last.normal &&
      raytraceEdgeSilhouetteEnabled === last.silhouette &&
      raytraceEdgeCreaseEnabled === last.crease &&
      raytraceEdgeBoundaryEnabled === last.boundary &&
      raytraceEdgeSilhouetteWidth === last.silhouetteWidth &&
      raytraceEdgeCreaseWidth === last.creaseWidth &&
      raytraceEdgeBoundaryWidth === last.boundaryWidth &&
      raytraceEdgeSoftness === last.softness
    ) {
      return;
    }

    lastEdgeSettingsRef.current = {
      enabled: raytraceEdgesEnabled,
      depth: raytraceEdgeDepthThreshold,
      normal: raytraceEdgeNormalThreshold,
      silhouette: raytraceEdgeSilhouetteEnabled,
      crease: raytraceEdgeCreaseEnabled,
      boundary: raytraceEdgeBoundaryEnabled,
      silhouetteWidth: raytraceEdgeSilhouetteWidth,
      creaseWidth: raytraceEdgeCreaseWidth,
      boundaryWidth: raytraceEdgeBoundaryWidth,
      softness: raytraceEdgeSoftness,
    };

    const rt = rayTracer as {
      setEdgeDetection?: (enabled: boolean, depth: number, normal: number) => void;
      setEdgeStyle?: (
        enableSilhouette: boolean, enableCrease: boolean, enableBoundary: boolean,
        silR: number, silG: number, silB: number, silA: number,
        crR: number, crG: number, crB: number, crA: number,
        bdR: number, bdG: number, bdB: number, bdA: number,
        silW: number, crW: number, bdW: number, softness: number,
      ) => void;
    };

    if (typeof rt.setEdgeDetection === "function") {
      rt.setEdgeDetection(
        raytraceEdgesEnabled,
        raytraceEdgeDepthThreshold,
        raytraceEdgeNormalThreshold,
      );
    }

    if (typeof rt.setEdgeStyle === "function") {
      // Use Fusion-style default colors (near-black, slightly cool)
      rt.setEdgeStyle(
        raytraceEdgeSilhouetteEnabled,
        raytraceEdgeCreaseEnabled,
        raytraceEdgeBoundaryEnabled,
        0.08, 0.08, 0.10, 1.0,  // silhouette color
        0.12, 0.12, 0.14, 1.0,  // crease color
        0.06, 0.06, 0.08, 1.0,  // boundary color
        raytraceEdgeSilhouetteWidth,
        raytraceEdgeCreaseWidth,
        raytraceEdgeBoundaryWidth,
        raytraceEdgeSoftness,
      );
    } else {
      logger.debug("gpu", "setEdgeStyle not available - WASM may need rebuild");
    }

    // Re-render to see the change
    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [
    raytraceEdgesEnabled, raytraceEdgeDepthThreshold, raytraceEdgeNormalThreshold,
    raytraceEdgeSilhouetteEnabled, raytraceEdgeCreaseEnabled, raytraceEdgeBoundaryEnabled,
    raytraceEdgeSilhouetteWidth, raytraceEdgeCreaseWidth, raytraceEdgeBoundaryWidth,
    raytraceEdgeSoftness,
    rayTracer, doRender,
  ]);

  // Apply path-tracer settings.
  //
  // The renderer is a real path tracer, so there is no SSAO pass any more —
  // multi-bounce GI computes contact occlusion correctly and a screen-space
  // proxy on top would double-darken concave corners. What is left to drive is
  // the depth ceiling and whether the Sobel edge overlay is drawn: edge lines
  // are a stylisation that fights photorealism, so "edges off" IS the photoreal
  // viewport mode.
  //
  // Depth itself escalates with accumulation inside the kernel, so the draft
  // frame stays interactive regardless of the ceiling set here.
  useEffect(() => {
    if (!rayTracer) return;

    const last = lastPathTraceSettingsRef.current;
    if (raytraceEdgesEnabled === last.stylize) return;
    lastPathTraceSettingsRef.current = { stylize: raytraceEdgesEnabled };

    const rt = rayTracer as {
      setPathTrace?: (maxDepth: number, stylize: boolean) => void;
    };
    if (typeof rt.setPathTrace !== "function") {
      logger.debug("gpu", "setPathTrace not available - WASM may need rebuild");
      return;
    }

    rt.setPathTrace(PATH_TRACE_MAX_DEPTH, raytraceEdgesEnabled);

    if (lastCameraStateRef.current) {
      doRender(lastCameraStateRef.current);
    }
  }, [raytraceEdgesEnabled, rayTracer, doRender]);

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
