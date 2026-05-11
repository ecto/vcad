import { useRef, useEffect, useMemo, useState, useCallback, Suspense } from "react";
import { Spherical, Vector3, Box3, Plane, Raycaster, Vector2, Quaternion, Matrix4, Color, TOUCH, PerspectiveCamera, WebGLRenderTarget, SRGBColorSpace, ACESFilmicToneMapping } from "three";

const isCoarsePointer =
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(pointer: coarse)").matches;
import { useThree, useFrame } from "@react-three/fiber";
import {
  findCoplanarTriangles,
  getEdgeEndpoints,
  getVertex,
} from "@/lib/sub-feature-geometry";
import {
  OrbitControls,
  GizmoHelper,
  GizmoViewport,
  Environment,
  Lightformer,
  ContactShadows,
} from "@react-three/drei";
import {
  EffectComposer,
  N8AO,
  Vignette,
} from "@react-three/postprocessing";
import type { OrbitControls as OrbitControlsImpl } from "three-stdlib";
import { GridPlane } from "./GridPlane";
import { SceneMesh, ImportedMesh } from "./SceneMesh";
import { BoundaryEdgeOverlay } from "./BoundaryEdgeOverlay";
import { useDebugOverlayStore } from "../stores/debug-overlay-store";
import { InspectedTriangleMarker } from "./TriangleInspector";
import { ClashMesh } from "./ClashMesh";
import { PreviewMesh } from "./PreviewMesh";
import { SketchPlane3D } from "./SketchPlane3D";
import { PlaneGizmo } from "./PlaneGizmo";
import { TransformGizmo } from "./TransformGizmo";
import { SelectionOverlay } from "./SelectionOverlay";
import { DimensionOverlay } from "./DimensionOverlay";
import { DfmAnnotations } from "./DfmAnnotations";
import { RayTracedViewportSync } from "./RayTracedViewport";
import { ParticipantCameraOverlay } from "./ParticipantCameraOverlay";
import {
  useEngineStore,
  useDocumentStore,
  useUiStore,
  useSketchStore,
  useParticipantStore,
  kernelToDisplay,
} from "@vcad/core";
import type { PartInfo, CameraGoal } from "@vcad/core";
import { useCameraControls } from "@/hooks/useCameraControls";
import { useTheme } from "@/hooks/useTheme";
import { useInputDeviceDetection } from "@/hooks/useInputDeviceDetection";
import { usePhysicsSimulation } from "@/hooks/usePhysicsSimulation";
import {
  useCameraSettingsStore,
  getEffectiveInputDevice,
  getActiveControlScheme,
} from "@/stores/camera-settings-store";
import {
  matchScrollBinding,
  getModifiersFromEvent,
  getOrbitControlsMouseButtons,
} from "@/lib/camera-controls";
import type { EvaluatedInstance } from "@vcad/engine";
import type {
  SceneSettings,
  EnvironmentPreset,
  Light as IrLight,
} from "@vcad/ir";
import { PcbScene } from "./electronics/pcb3d/PcbScene";
import { usePcbCamera } from "./electronics/pcb3d/usePcbCamera";
import { BG_DARK, BG_LIGHT } from "./Viewport";
import { useXRPresenting } from "@/stores/xr-store";
import { XRSceneTransform } from "./xr/XRSceneTransform";
import { XRGestures } from "./xr/XRGestures";
import { XRPresence } from "./xr/XRPresence";

// Reused per-frame in the participant-sync hook (Lock + Follow).
const _syncGoalPos = new Vector3();
const _syncGoalTarget = new Vector3();

/**
 * Flip Lock mode back to Free when the user grabs the camera themselves.
 * Follow mode is intentionally *not* broken here — the user is supposed to
 * be able to orbit around the followed participant's camera while still
 * tracking it. Cheap to call on every orbit start / pointer down / wheel
 * tick — noop unless we're actually locked.
 */
function breakLockOnUserInput(): void {
  const ui = useUiStore.getState();
  if (ui.followMode !== "lock") return;
  ui.setFollowMode("free");
  ui.setFollowingParticipant(null);
}

// Map IR environment presets to drei preset names
const ENVIRONMENT_PRESET_MAP: Record<EnvironmentPreset, string> = {
  studio: "studio",
  warehouse: "warehouse",
  apartment: "apartment",
  park: "park",
  city: "city",
  dawn: "dawn",
  night: "night",
  sunset: "sunset",
  forest: "forest",
  neutral: "studio", // drei has no "neutral" preset; studio is closest
};

// Effective scene settings type (all fields required)
interface EffectiveSceneSettings {
  environment: NonNullable<SceneSettings["environment"]>;
  lights: IrLight[];
  background: NonNullable<SceneSettings["background"]>;
  postProcessing: NonNullable<SceneSettings["postProcessing"]>;
}

// Default scene settings (smart defaults).
// Background matches the surrounding UI chrome so the viewport blends
// into the app. The `<Environment>` below still produces IBL through
// `scene.environment`, independent of `scene.background`, so metallic
// reflections stay intact.
function buildDefaultSceneSettings(isDark: boolean): EffectiveSceneSettings {
  return {
    // No HDR preset — the default scene renders a procedural room
    // IBL via Lightformers below. Users can opt into a preset via the
    // Scene inspector.
    environment: { type: "None" },
    lights: [
      {
        id: "key",
        kind: { type: "Directional", direction: { x: 0.5, y: -0.8, z: 0.4 } },
        color: [1, 0.98, 0.95],
        intensity: isDark ? 1.4 : 1.2,
        castShadow: true,
      },
      {
        id: "fill",
        kind: { type: "Directional", direction: { x: -0.3, y: -0.4, z: -0.2 } },
        color: [0.95, 0.97, 1.0],
        intensity: isDark ? 0.5 : 0.4,
      },
      {
        id: "rim",
        kind: { type: "Directional", direction: { x: -0.5, y: -0.2, z: 0.5 } },
        color: [1, 1, 1],
        intensity: isDark ? 0.35 : 0.2,
      },
    ],
    // No background in defaults — the render path below paints the
    // viewport flat with the UI chrome color when the doc hasn't
    // overridden it.
    background: { type: "Solid", color: [0, 0, 0] },
    postProcessing: isDark
      ? {
          ambientOcclusion: { enabled: true, intensity: 1.8, radius: 0.5 },
          vignette: { enabled: true, offset: 0.5, darkness: 0.15 },
        }
      : {
          ambientOcclusion: { enabled: true, intensity: 1.5, radius: 0.5 },
          vignette: { enabled: true, offset: 0.5, darkness: 0.1 },
        },
  };
}

// Compute effective scene settings (merge document settings with defaults)
function getEffectiveSceneSettings(scene: SceneSettings | undefined, isDark: boolean): EffectiveSceneSettings {
  const base = buildDefaultSceneSettings(isDark);

  if (!scene) return base;

  return {
    environment: scene.environment ?? base.environment,
    lights: scene.lights ?? base.lights,
    background: scene.background ?? base.background,
    postProcessing: scene.postProcessing ?? base.postProcessing,
  };
}

// Convert IR light direction to Three.js position (lights point FROM position TO origin)
function lightDirectionToPosition(direction: { x: number; y: number; z: number }, distance = 100): [number, number, number] {
  // Negate and scale the direction to get a position
  return [-direction.x * distance, -direction.y * distance, -direction.z * distance];
}

function getInstanceSelectionId(inst: EvaluatedInstance): string {
  const instance = inst as {
    id?: string;
    instanceId?: string;
    partDefId?: string;
  };
  return instance.id ?? instance.instanceId ?? instance.partDefId ?? "";
}

// Compute a "level" quaternion (zero roll) for a camera at `eye` looking at `target`
const _tempMatrix = new Matrix4();
const _tempForward = new Vector3();
const _tempRight = new Vector3();
const _tempUp = new Vector3();
const _tempBack = new Vector3();
function computeLevelQuaternion(
  eye: Vector3,
  target: Vector3,
  out: Quaternion,
): Quaternion {
  // Forward direction (camera looks down -Z in its local space)
  _tempForward.subVectors(target, eye).normalize();

  // Standard Three.js Y-up (rotation group handles kernel Z-up → display Y-up)
  const worldUp = new Vector3(0, 1, 0);
  const dot = Math.abs(_tempForward.dot(worldUp));

  if (dot > 0.999) {
    // Looking straight up or down Y - use world Z as the reference
    _tempRight.crossVectors(new Vector3(0, 0, 1), _tempForward).normalize();
  } else {
    // Normal case: right = forward × worldUp
    _tempRight.crossVectors(_tempForward, worldUp).normalize();
  }

  // Compute up for right-handed system: up = right × forward
  _tempUp.crossVectors(_tempRight, _tempForward);

  // Back direction (camera +Z) = -forward
  _tempBack.copy(_tempForward).negate();

  // Build rotation matrix (camera convention: +X right, +Y up, +Z back)
  _tempMatrix.makeBasis(_tempRight, _tempUp, _tempBack);
  out.setFromRotationMatrix(_tempMatrix);

  return out;
}

/** Render boundary-edge overlays for every part's mesh when the debug
 * flag is on. Registers Ctrl+Shift+B as a global toggle. */
function DebugBoundaryOverlays() {
  const show = useDebugOverlayStore((s) => s.showBoundaryEdges);
  const toggle = useDebugOverlayStore((s) => s.toggleBoundaryEdges);
  const scene = useEngineStore((s) => s.scene);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && (e.key === "B" || e.key === "b")) {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [toggle]);

  if (!show || !scene) return null;
  return (
    <>
      {scene.parts.map((p, i) => {
        if (!p.mesh.positions.length) return null;
        return (
          <BoundaryEdgeOverlay
            key={`boundary-${i}`}
            positions={p.mesh.positions}
            indices={p.mesh.indices}
          />
        );
      })}
    </>
  );
}

/** Triangle picker R3F component: registers the Ctrl+Shift+T hotkey and
 *  renders the highlight marker inside the scene rotation group. The
 *  DOM info panel lives outside the Canvas — see `TriangleInspectionPanel`
 *  mounted in the Viewport parent. */
function DebugTriangleInspector() {
  const inspectEnabled = useDebugOverlayStore((s) => s.inspectTriangles);
  const toggle = useDebugOverlayStore((s) => s.toggleInspectTriangles);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && (e.key === "T" || e.key === "t")) {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [toggle]);

  if (!inspectEnabled) return null;
  return <InspectedTriangleMarker />;
}

export function ViewportContent({ mode = "3d" }: { mode?: "3d" | "pcb" }) {
  useCameraControls();
  useInputDeviceDetection();
  usePhysicsSimulation();

  const isPcbMode = mode === "pcb";

  // Track camera motion to disable expensive effects during animation/orbit
  const [isCameraMoving, setIsCameraMoving] = useState(false);

  const engineReady = useEngineStore((s) => s.engineReady);
  const scene = useEngineStore((s) => s.scene);
  const previewMesh = useEngineStore((s) => s.previewMesh);
  const parts = useDocumentStore((s) => s.parts);
  const docInstances = useDocumentStore((s) => s.document.instances);
  const docPartDefs = useDocumentStore((s) => s.document.partDefs);
  const docRoots = useDocumentStore((s) => s.document.roots);
  const docScene = useDocumentStore((s) => s.document.scene);

  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const selection = useUiStore((s) => s.selection);
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const setOrbiting = useUiStore((s) => s.setOrbiting);
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const sketchActive = useSketchStore((s) => s.active);
  const xrPresenting = useXRPresenting();
  const orbitRef = useRef<OrbitControlsImpl>(null);
  usePcbCamera(orbitRef, isPcbMode);
  const { camera, invalidate } = useThree();
  const { isDark } = useTheme();

  // Camera uses standard Three.js Y-up; the rotation group on geometry handles Z-up → Y-up
  // (no need to change camera.up from default [0,1,0])

  // Camera settings from store
  const cameraSettings = useCameraSettingsStore();
  const controlScheme = getActiveControlScheme(cameraSettings);
  const effectiveDevice = getEffectiveInputDevice(cameraSettings);
  const { zoomBehavior, orbitMomentum } = cameraSettings;

  // Raycaster for zoom-to-cursor
  const raycasterRef = useRef(new Raycaster());
  const mouseRef = useRef(new Vector2());

  // Stable ref for invalidate so wheel effect closure always has current value
  const invalidateRef = useRef(invalidate);
  invalidateRef.current = invalidate;

  // Reusable objects to avoid GC pressure (wheel fires at 60+ Hz)
  const sphericalRef = useRef(new Spherical());
  const offsetRef = useRef(new Vector3());
  const velocityRef = useRef({ theta: 0, phi: 0 });
  const animatingRef = useRef(false);
  const cursorPointRef = useRef(new Vector3()); // Reused in zoom-to-cursor

  // Target animation for orbit focus
  const targetGoalRef = useRef(new Vector3());
  const distanceGoalRef = useRef<number | null>(null);
  const isAnimatingTargetRef = useRef(false);

  // Camera position goal for face-aligned view
  const cameraPositionGoalRef = useRef<Vector3 | null>(null);

  // Quaternion ref for smooth orientation interpolation
  const goalQuatRef = useRef(new Quaternion());

  // Cancel any in-flight camera animation so user input takes over immediately.
  const cancelCameraAnimation = useCallback(() => {
    if (!isAnimatingTargetRef.current) return;
    isAnimatingTargetRef.current = false;
    distanceGoalRef.current = null;
    cameraPositionGoalRef.current = null;
    if (orbitRef.current) orbitRef.current.enabled = true;
    setIsCameraMoving(false);
  }, []);

  // Build mapping from root index to instance ID (for assembly mode rendering with legacy parts)
  const rootIndexToInstanceId = useMemo(() => {
    const mapping = new Map<number, string>();
    if (!docInstances || !docPartDefs) return mapping;

    // Build root NodeId -> root index lookup
    const rootToIndex = new Map<number, number>();
    docRoots.forEach((entry, idx) => {
      rootToIndex.set(entry.root, idx);
    });

    // Map each instance to its corresponding root index
    for (const instance of docInstances) {
      const partDef = docPartDefs[instance.partDefId];
      if (!partDef) continue;
      const rootIdx = rootToIndex.get(partDef.root);
      if (rootIdx !== undefined) {
        mapping.set(rootIdx, instance.id);
      }
    }
    return mapping;
  }, [docInstances, docPartDefs, docRoots]);

  // Check if a part at given index is selected (handles both part IDs and instance IDs)
  const isPartSelected = useCallback(
    (partId: string, partIndex: number): boolean => {
      // Direct part ID match
      if (selectedPartIds.has(partId)) return true;
      // Instance ID match (for assembly mode)
      const instanceId = rootIndexToInstanceId.get(partIndex);
      if (instanceId && selectedPartIds.has(instanceId)) return true;
      return false;
    },
    [selectedPartIds, rootIndexToInstanceId],
  );

  // Calculate center and size of the current selection — handles parts,
  // instances, and sub-features (face / edge / vertex). Returns null when
  // the selection is empty or none of its items resolve to geometry.
  //
  // Two modes:
  //   "fit"      — frame the bbox by the current FOV (existing behavior).
  //   "pan-only" — re-target to the center but keep the current zoom.
  //                Used for sub-feature selections so a vertex pick on a
  //                big sphere doesn't dive into a 1mm close-up that loses
  //                all context. The highlight + property panel are the
  //                primary feedback there; the camera just centers it.
  const selectionInfo = useMemo(() => {
    if (selection.length === 0 || !scene) return null;

    const hasSubFeature = selection.some(
      (it) => it.kind === "face" || it.kind === "edge" || it.kind === "vertex",
    );

    const box = new Box3();
    const tempVec = new Vector3();
    let hasPoints = false;

    /** Look up an EvaluatedPart's mesh by partId. */
    const meshFor = (partId: string) => {
      const idx = parts.findIndex((p) => p.id === partId);
      if (idx < 0) return null;
      return scene.parts[idx]?.mesh ?? null;
    };

    for (const item of selection) {
      switch (item.kind) {
        case "part": {
          // Assembly path: instance with this id.
          if (scene.instances && scene.instances.length > 0) {
            for (const inst of scene.instances) {
              const instanceSelectionId = getInstanceSelectionId(inst);
              if (instanceSelectionId !== item.id) continue;
              const positions = inst.mesh.positions;
              const t = inst.transform ?? {
                translation: { x: 0, y: 0, z: 0 },
                rotation: { x: 0, y: 0, z: 0 },
                scale: { x: 1, y: 1, z: 1 },
              };
              for (let i = 0; i < positions.length; i += 3) {
                tempVec.set(
                  positions[i]! * t.scale.x + t.translation.x,
                  positions[i + 1]! * t.scale.y + t.translation.y,
                  positions[i + 2]! * t.scale.z + t.translation.z,
                );
                box.expandByPoint(tempVec);
                hasPoints = true;
              }
            }
          }
          // Legacy path: full part mesh. Also covers instances exposed as
          // parts via isPartSelected — keep the existing index-based walk
          // so the same id can match either path.
          parts.forEach((part, index) => {
            if (!isPartSelected(part.id, index)) return;
            if (part.id !== item.id) return;
            const evalPart = scene.parts[index];
            if (!evalPart) return;
            const positions = evalPart.mesh.positions;
            for (let i = 0; i < positions.length; i += 3) {
              tempVec.set(positions[i]!, positions[i + 1]!, positions[i + 2]!);
              box.expandByPoint(tempVec);
              hasPoints = true;
            }
          });
          break;
        }
        case "face": {
          const mesh = meshFor(item.partId);
          if (!mesh) break;
          const tris = findCoplanarTriangles(mesh, item.faceIndex);
          for (const t of tris) {
            for (let v = 0; v < 3; v++) {
              const idx = mesh.indices[t * 3 + v]!;
              tempVec.set(
                mesh.positions[idx * 3]!,
                mesh.positions[idx * 3 + 1]!,
                mesh.positions[idx * 3 + 2]!,
              );
              box.expandByPoint(tempVec);
              hasPoints = true;
            }
          }
          break;
        }
        case "edge": {
          const mesh = meshFor(item.partId);
          if (!mesh) break;
          const { a, b } = getEdgeEndpoints(mesh, item.edgeId);
          box.expandByPoint(a);
          box.expandByPoint(b);
          hasPoints = true;
          break;
        }
        case "vertex": {
          const mesh = meshFor(item.partId);
          if (!mesh) break;
          const p = getVertex(mesh, item.vertexId);
          box.expandByPoint(p);
          hasPoints = true;
          break;
        }
        case "segment":
        case "constraint":
          // Sketch-mode entities have their own framing path; skip here.
          break;
      }
    }

    if (!hasPoints) return null;
    const kernelCenter = new Vector3();
    box.getCenter(kernelCenter);
    const size = new Vector3();
    box.getSize(size);
    // Bounding-sphere radius — independent of orbit angle, safe for any
    // view. Single-vertex selections have zero size; clamp to 1mm so the
    // camera doesn't dive in until the near-plane clips.
    const radius = Math.max(size.length() * 0.5, 1);
    // Kernel Z-up center → display Y-up: (x, y, z) → (x, z, -y).
    const center = new Vector3(kernelCenter.x, kernelCenter.z, -kernelCenter.y);
    const mode: "fit" | "pan-only" = hasSubFeature ? "pan-only" : "fit";
    return { center, radius, mode };
  }, [selection, scene, parts, isPartSelected]);

  // Animate orbit target to selection center. For part selections we also
  // tighten distance to fit the bounding sphere; for sub-features we keep
  // the current zoom — picking a vertex on a sphere shouldn't bury you in
  // a 1mm close-up that loses all context.
  useEffect(() => {
    if (selectionInfo && !isDraggingGizmo) {
      targetGoalRef.current.copy(selectionInfo.center);
      if (selectionInfo.mode === "fit") {
        const padding = 1.2;
        if (camera instanceof PerspectiveCamera) {
          const vFov = (camera.fov * Math.PI) / 180;
          const hFov = 2 * Math.atan(Math.tan(vFov / 2) * camera.aspect);
          const fitFov = Math.min(vFov, hFov);
          const distance =
            (selectionInfo.radius / Math.sin(fitFov / 2)) * padding;
          distanceGoalRef.current = Math.max(camera.near * 2, distance);
        } else {
          distanceGoalRef.current = selectionInfo.radius * 3 * padding;
        }
      } else {
        // pan-only — leave the distance goal untouched. The lerp loop
        // honors a null distanceGoal by skipping its update.
        distanceGoalRef.current = null;
      }
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    }
  }, [selectionInfo, isDraggingGizmo, camera]);

  // Smooth target and distance animation
  useFrame(() => {
    if (!isAnimatingTargetRef.current || !orbitRef.current) return;

    const target = orbitRef.current.target;
    const targetGoal = targetGoalRef.current;
    const distanceGoal = distanceGoalRef.current;
    const cameraPositionGoal = cameraPositionGoalRef.current;
    const lerpFactor = 0.1;

    // Animate target position
    target.lerp(targetGoal, lerpFactor);

    // Animate camera position if we have a specific goal (face-aligned view)
    if (cameraPositionGoal !== null) {
      camera.position.lerp(cameraPositionGoal, lerpFactor);
      // Use quaternion slerp for smooth orientation (avoids lookAt roll jumps)
      camera.quaternion.slerp(goalQuatRef.current, lerpFactor);
      // Sync target for when OrbitControls resumes
      target.copy(targetGoal);
    } else if (distanceGoal !== null) {
      // Animate camera distance only (keep current direction)
      const offset = offsetRef.current.subVectors(camera.position, target);
      const currentDist = offset.length();
      const newDist = currentDist + (distanceGoal - currentDist) * lerpFactor;
      offset.normalize().multiplyScalar(newDist);
      camera.position.copy(target).add(offset);
    }

    // Stop animating when close enough
    const targetDone = target.distanceTo(targetGoal) < 0.01;
    const distanceDone =
      distanceGoal === null ||
      Math.abs(
        offsetRef.current.subVectors(camera.position, target).length() -
          distanceGoal,
      ) < 0.1;
    const positionDone =
      cameraPositionGoal === null ||
      camera.position.distanceTo(cameraPositionGoal) < 0.1;

    if (targetDone && distanceDone && positionDone) {
      target.copy(targetGoal);
      if (cameraPositionGoal) {
        camera.position.copy(cameraPositionGoal);
        camera.quaternion.copy(goalQuatRef.current);
      }
      // Re-enable OrbitControls now that animation is complete
      if (orbitRef.current) orbitRef.current.enabled = true;
      isAnimatingTargetRef.current = false;
      distanceGoalRef.current = null;
      cameraPositionGoalRef.current = null;
      // Re-enable expensive effects now that animation is complete
      setIsCameraMoving(false);
    } else {
      // Request next frame to continue animation (demand mode)
      invalidate();
    }
  });

  // Demand rendering never kicks a frame on its own when the AI moves
  // its camera while we're in Follow/Lock (in Lock we skip drawing the
  // AI's frustum, so there's no other subscriber to invalidate for us;
  // in Follow the frustum overlay kicks itself but we still need a frame
  // to re-aim the user camera). Subscribe to both stores and invalidate
  // whenever a mode-relevant change lands — the useFrame below
  // self-sustains after the first fire.
  useEffect(() => {
    const kick = () => {
      if (useUiStore.getState().followMode !== "free") invalidate();
    };
    const unsubUi = useUiStore.subscribe(kick);
    const unsubPart = useParticipantStore.subscribe(kick);
    return () => {
      unsubUi();
      unsubPart();
    };
  }, [invalidate]);

  // Per-frame participant-camera sync (Follow + Lock).
  //
  //  - Lock   : hard-copy the followed participant's camera every frame
  //             (kernel Z-up → display Y-up). No interpolation — the user
  //             sees exactly what the AI sees. Preempts any in-flight
  //             focus/snap animation so lock stays authoritative.
  //  - Follow : keep the user's eye position; lerp the orbit target
  //             toward the AI's eye position so the user's view rotates
  //             to frame the AI's frustum wireframe. User keeps orbit +
  //             zoom, but always rotates around / looks at the AI.
  useFrame(() => {
    const { followMode, followingParticipantId } = useUiStore.getState();
    if (followMode === "free" || !followingParticipantId) return;
    const participant =
      useParticipantStore.getState().participants.get(followingParticipantId);
    if (!participant?.camera) return;

    const [px, py, pz] = kernelToDisplay(participant.camera.position);
    const goalPos = _syncGoalPos.set(px, py, pz);

    if (followMode === "lock") {
      // Lock overrides any in-flight focus/snap animation.
      if (isAnimatingTargetRef.current) cancelCameraAnimation();
      const [tx, ty, tz] = kernelToDisplay(participant.camera.target);
      const goalTarget = _syncGoalTarget.set(tx, ty, tz);
      camera.position.copy(goalPos);
      if (orbitRef.current) {
        orbitRef.current.target.copy(goalTarget);
        orbitRef.current.update();
      } else {
        camera.lookAt(goalTarget);
      }
      invalidate();
      return;
    }

    // Follow: aim the user's look-at at the AI's eye (frustum apex).
    // Don't fight an in-flight focus animation the user explicitly
    // triggered — let it finish, then follow resumes next frame.
    if (isAnimatingTargetRef.current) return;
    const lerp = 0.15;
    if (orbitRef.current) {
      orbitRef.current.target.lerp(goalPos, lerp);
      orbitRef.current.update();
      const targetDelta =
        orbitRef.current.target.distanceToSquared(goalPos);
      if (targetDelta > 1e-4) invalidate();
    } else {
      camera.lookAt(goalPos);
    }
  });

  // Wheel handler with configurable control schemes
  useEffect(() => {
    const controls = orbitRef.current;
    const domElement = controls?.domElement;
    if (!domElement) return;

    const dampingFactor = 1.0;
    const friction = 0.0;

    // Batch wheel events to avoid multiple renders per frame
    let pendingUpdate = false;
    const scheduleUpdate = () => {
      if (pendingUpdate) return;
      pendingUpdate = true;
      requestAnimationFrame(() => {
        pendingUpdate = false;
        controls.update();
        invalidateRef.current();
      });
    };

    const animate = () => {
      // Check if momentum is enabled
      if (!orbitMomentum) {
        animatingRef.current = false;
        velocityRef.current.theta = 0;
        velocityRef.current.phi = 0;
        return;
      }

      const vel = velocityRef.current;
      // Stop animating when velocity is negligible
      if (Math.abs(vel.theta) < 0.0001 && Math.abs(vel.phi) < 0.0001) {
        animatingRef.current = false;
        vel.theta = 0;
        vel.phi = 0;
        return;
      }

      const target = controls.target;
      const offset = offsetRef.current.subVectors(camera.position, target);
      const spherical = sphericalRef.current.setFromVector3(offset);

      // Apply fraction of velocity
      spherical.theta += vel.theta * dampingFactor;
      spherical.phi += vel.phi * dampingFactor;

      // Clamp polar angle to avoid flipping
      spherical.phi = Math.max(0.01, Math.min(Math.PI - 0.01, spherical.phi));

      // Decay velocity
      vel.theta *= friction;
      vel.phi *= friction;

      offset.setFromSpherical(spherical);
      camera.position.copy(target).add(offset);
      camera.lookAt(target);
      controls.update();
      invalidateRef.current();

      requestAnimationFrame(animate);
    };

    // Zoom implementation with zoom-to-cursor support
    const performZoom = (e: WheelEvent, dx: number, dy: number) => {
      const baseSpeed = 0.002 * zoomBehavior.sensitivity;
      let delta = -(Math.abs(dy) > Math.abs(dx) ? dy : dx) * baseSpeed;

      if (zoomBehavior.invertDirection) {
        delta = -delta;
      }

      const target = controls.target;
      const offset = offsetRef.current.subVectors(camera.position, target);
      const distance = offset.length();
      const newDistance = Math.max(1, distance * (1 + delta));

      if (zoomBehavior.zoomTowardsCursor) {
        // Zoom toward cursor position
        const rect = domElement.getBoundingClientRect();
        mouseRef.current.x = ((e.clientX - rect.left) / rect.width) * 2 - 1;
        mouseRef.current.y = -((e.clientY - rect.top) / rect.height) * 2 + 1;

        raycasterRef.current.setFromCamera(mouseRef.current, camera);

        // Project cursor to a plane at the current target distance (reuse Vector3 to avoid GC)
        const cursorPoint = cursorPointRef.current;
        raycasterRef.current.ray.at(distance, cursorPoint);

        // Interpolate target toward cursor based on zoom amount
        const zoomFactor = 1 - newDistance / distance;
        target.lerp(cursorPoint, zoomFactor * 0.5);
      }

      offset.normalize().multiplyScalar(newDistance);
      camera.position.copy(target).add(offset);
      scheduleUpdate();
    };

    // Pan implementation
    const performPan = (dx: number, dy: number) => {
      const target = controls.target;
      const offset = offsetRef.current.subVectors(camera.position, target);
      const distance = offset.length();

      // Scale pan speed by distance so it feels consistent at any zoom
      const panSpeed = distance * 0.002;

      // Get camera's right and up vectors in world space
      const right = new Vector3();
      const up = new Vector3();
      camera.matrix.extractBasis(right, up, new Vector3());

      // Calculate pan offset: drag to pull the view
      const panOffset = right
        .multiplyScalar(dx * panSpeed)
        .add(up.multiplyScalar(-dy * panSpeed));

      // Move both camera and target by the same amount
      camera.position.add(panOffset);
      target.add(panOffset);
      scheduleUpdate();
    };

    // Orbit implementation
    const performOrbit = (dx: number, dy: number, withMomentum: boolean) => {
      // OrbitControls formula: viewport height = 2π radians
      const rotateSpeed = (2 * Math.PI) / domElement.clientHeight;

      if (withMomentum && orbitMomentum) {
        // Accumulate velocity for momentum animation
        velocityRef.current.theta += dx * rotateSpeed;
        velocityRef.current.phi += dy * rotateSpeed;

        // Start animation loop if not already running
        if (!animatingRef.current) {
          animatingRef.current = true;
          requestAnimationFrame(animate);
        }
      } else {
        // Immediate orbit without momentum
        const target = controls.target;
        const offset = offsetRef.current.subVectors(camera.position, target);
        const spherical = sphericalRef.current.setFromVector3(offset);

        spherical.theta += dx * rotateSpeed;
        spherical.phi += dy * rotateSpeed;
        spherical.phi = Math.max(0.01, Math.min(Math.PI - 0.01, spherical.phi));

        offset.setFromSpherical(spherical);
        camera.position.copy(target).add(offset);
        camera.lookAt(target);
        scheduleUpdate();
      }
    };

    const handleWheel = (e: WheelEvent) => {
      e.preventDefault();
      e.stopPropagation();

      // User input overrides any in-flight focus/snap camera animation.
      cancelCameraAnimation();
      breakLockOnUserInput();

      // Normalize deltaMode: 0=pixels, 1=lines, 2=pages
      let dx = e.deltaX;
      let dy = e.deltaY;
      if (e.deltaMode === 1) {
        dx *= 16;
        dy *= 16;
      } // lines → pixels
      if (e.deltaMode === 2) {
        dx *= 100;
        dy *= 100;
      } // pages → pixels

      // Get modifiers and match to action
      const modifiers = getModifiersFromEvent(e);
      const action = matchScrollBinding(
        controlScheme.scrollBindings,
        modifiers,
        effectiveDevice,
      );

      // Check if trackpad orbit is enabled for this scheme
      const useTrackpadOrbit =
        controlScheme.trackpadOrbitEnabled && effectiveDevice === "trackpad";

      switch (action) {
        case "zoom":
          performZoom(e, dx, dy);
          break;
        case "pan":
          performPan(dx, dy);
          break;
        case "orbit":
          performOrbit(dx, dy, useTrackpadOrbit);
          break;
        default:
          // No action configured for this modifier combination
          break;
      }
    };

    domElement.addEventListener("wheel", handleWheel, { passive: false });
    return () => domElement.removeEventListener("wheel", handleWheel);
  }, [
    camera,
    controlScheme,
    effectiveDevice,
    zoomBehavior,
    orbitMomentum,
    cancelCameraAnimation,
  ]);

  // Disable raycasting and expensive effects during orbit for performance
  useEffect(() => {
    const controls = orbitRef.current;
    if (!controls) return;
    const domElement = controls.domElement;

    const handleStart = () => {
      setOrbiting(true);
      setIsCameraMoving(true);
      // User took the wheel — drop out of Lock so we stop fighting for
      // camera control every frame.
      breakLockOnUserInput();
    };
    const handleEnd = () => {
      setOrbiting(false);
      setIsCameraMoving(false);
    };
    // Any mouse/touch press cancels an in-flight camera animation so user
    // drag takes over immediately (covers cases where OrbitControls is
    // disabled during snap/face animations and "start" wouldn't fire).
    const handlePointerDown = () => {
      cancelCameraAnimation();
      breakLockOnUserInput();
    };

    controls.addEventListener("start", handleStart);
    controls.addEventListener("end", handleEnd);
    domElement?.addEventListener("pointerdown", handlePointerDown);

    return () => {
      controls.removeEventListener("start", handleStart);
      controls.removeEventListener("end", handleEnd);
      domElement?.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [setOrbiting, cancelCameraAnimation]);

  // Double-click on empty canvas: fit camera to the scene (same handler as
  // boot's auto-fit + the View → Fit menu). Was previously "reset to the
  // hardcoded initial pose," which would hide the model whenever the
  // geometry didn't live near (0, 0, 0).
  useEffect(() => {
    const controls = orbitRef.current;
    const domElement = controls?.domElement;
    if (!domElement) return;

    const handleDoubleClick = () => {
      // Only fit when nothing is selected — with selection, the existing
      // selection-fit useEffect already does the right thing on the click.
      if (useUiStore.getState().selectedPartIds.size > 0) return;
      window.dispatchEvent(new CustomEvent("vcad:camera-fit"));
    };

    domElement.addEventListener("dblclick", handleDoubleClick);
    return () => domElement.removeEventListener("dblclick", handleDoubleClick);
  }, []);

  // Publish cursor world position (Z-up) to UiStore for the status bar readout.
  // Raycasts pointer against the ground plane (kernel Z=0, display Y=0) via the
  // Three.js -90°X scene rotation. Throttled to one update per animation frame.
  useEffect(() => {
    const controls = orbitRef.current;
    const domElement = controls?.domElement;
    if (!domElement) return;

    const groundPlane = new Plane(new Vector3(0, 1, 0), 0);
    const pointer = new Vector2();
    const raycaster = new Raycaster();
    const hit = new Vector3();
    let pending = false;
    let lastEvent: PointerEvent | null = null;

    const flush = () => {
      pending = false;
      const e = lastEvent;
      if (!e) return;
      const rect = domElement.getBoundingClientRect();
      pointer.x = ((e.clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((e.clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);
      const intersected = raycaster.ray.intersectPlane(groundPlane, hit);
      if (!intersected) {
        useUiStore.getState().setCursorWorld(null);
        return;
      }
      // Display (Y-up) → kernel (Z-up): (dx, dy, dz) → (dx, -dz, dy)
      useUiStore.getState().setCursorWorld({
        x: hit.x,
        y: -hit.z,
        z: hit.y,
      });
    };

    const handlePointerMove = (e: PointerEvent) => {
      lastEvent = e;
      if (pending) return;
      pending = true;
      requestAnimationFrame(flush);
    };

    const handlePointerLeave = () => {
      lastEvent = null;
      useUiStore.getState().setCursorWorld(null);
    };

    domElement.addEventListener("pointermove", handlePointerMove);
    domElement.addEventListener("pointerleave", handlePointerLeave);
    return () => {
      domElement.removeEventListener("pointermove", handlePointerMove);
      domElement.removeEventListener("pointerleave", handlePointerLeave);
      useUiStore.getState().setCursorWorld(null);
    };
  }, [camera]);

  // Face selection: swing camera perpendicular to the face, framing its
  // actual extent instead of a fixed-distance fallback.
  useEffect(() => {
    const handleFaceSelected = (
      e: CustomEvent<{
        normal: { x: number; y: number; z: number };
        centroid: { x: number; y: number; z: number };
        vertices?: { x: number; y: number; z: number }[];
      }>,
    ) => {
      const { normal, centroid, vertices } = e.detail;

      // Both centroid and normal arrive in kernel (Z-up) space. The camera
      // and OrbitControls live in display (Y-up) space — convert both via
      // the (x, y, z) → (x, z, -y) mapping that the scene's rotation group
      // applies to geometry.
      const wNormal = new Vector3(normal.x, normal.z, -normal.y).normalize();
      const wCentroid = new Vector3(centroid.x, centroid.z, -centroid.y);

      // Pick view distance from the face's actual U/V extent when we have
      // vertices, using the camera FOV so the face fills the viewport with
      // a 20% padding. Falls back to 60mm when vertices weren't supplied
      // (e.g. axis-aligned plane gizmo entry).
      let viewDistance = 60;
      if (vertices && vertices.length >= 3) {
        // Build an orthonormal U/V basis on the face.
        const tmp = new Vector3(0, 1, 0);
        if (Math.abs(wNormal.dot(tmp)) > 0.95) tmp.set(1, 0, 0);
        const uDir = new Vector3().crossVectors(tmp, wNormal).normalize();
        const vDir = new Vector3().crossVectors(wNormal, uDir).normalize();
        let minU = Infinity, maxU = -Infinity;
        let minV = Infinity, maxV = -Infinity;
        const offset = new Vector3();
        for (const k of vertices) {
          const wp = new Vector3(k.x, k.z, -k.y);
          offset.subVectors(wp, wCentroid);
          const u = offset.dot(uDir);
          const v = offset.dot(vDir);
          if (u < minU) minU = u;
          if (u > maxU) maxU = u;
          if (v < minV) minV = v;
          if (v > maxV) maxV = v;
        }
        const width = Math.max(1, maxU - minU);
        const height = Math.max(1, maxV - minV);
        const padding = 1.2;
        if (camera instanceof PerspectiveCamera) {
          const vFov = (camera.fov * Math.PI) / 180;
          const hFov = 2 * Math.atan(Math.tan(vFov / 2) * camera.aspect);
          const distV = (height / 2) / Math.tan(vFov / 2);
          const distH = (width / 2) / Math.tan(hFov / 2);
          viewDistance = Math.max(distV, distH, 10) * padding;
        } else {
          viewDistance = Math.max(width, height, 10) * padding;
        }
      }

      const cameraPos = new Vector3(
        wCentroid.x + wNormal.x * viewDistance,
        wCentroid.y + wNormal.y * viewDistance,
        wCentroid.z + wNormal.z * viewDistance,
      );

      targetGoalRef.current.copy(wCentroid);
      cameraPositionGoalRef.current = cameraPos;
      distanceGoalRef.current = viewDistance;

      // Compute goal quaternion for smooth orientation interpolation (zero roll)
      computeLevelQuaternion(cameraPos, wCentroid, goalQuatRef.current);

      // Disable OrbitControls during animation so it doesn't fight with our quaternion
      if (orbitRef.current) orbitRef.current.enabled = false;
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };

    window.addEventListener(
      "vcad:face-selected",
      handleFaceSelected as EventListener,
    );
    return () =>
      window.removeEventListener(
        "vcad:face-selected",
        handleFaceSelected as EventListener,
      );
  }, [camera]);

  // Snap view: animate camera to predefined positions
  useEffect(() => {
    const CAMERA_DISTANCE = 80;
    // Snap views in Three.js Y-up display space
    // Kernel→display: (x,y,z) → (x, z, -y)
    const SNAP_VIEWS: Record<string, [number, number, number]> = {
      front: [0, 0, CAMERA_DISTANCE],     // Kernel +Y → display -Z, so camera at +Z looks at front
      back: [0, 0, -CAMERA_DISTANCE],
      right: [CAMERA_DISTANCE, 0, 0],
      left: [-CAMERA_DISTANCE, 0, 0],
      top: [0, CAMERA_DISTANCE, 0],       // Kernel +Z → display +Y, looking down
      bottom: [0, -CAMERA_DISTANCE, 0],
      iso: [50, 50, 50],
      hero: [60, 45, 60],
    };

    const handleSnapView = (e: CustomEvent<string>) => {
      const view = e.detail;
      const pos = SNAP_VIEWS[view];
      if (!pos) return;

      // Animate to the new position
      const cameraPos = new Vector3(pos[0], pos[1], pos[2]);
      const targetVec = new Vector3(0, 0, 0);
      targetGoalRef.current.copy(targetVec);
      cameraPositionGoalRef.current = cameraPos;
      distanceGoalRef.current = null; // Don't override distance when we have explicit position

      // Compute goal quaternion for smooth orientation interpolation (zero roll)
      computeLevelQuaternion(cameraPos, targetVec, goalQuatRef.current);

      // Disable OrbitControls during animation
      if (orbitRef.current) orbitRef.current.enabled = false;
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };

    window.addEventListener("vcad:snap-view", handleSnapView as EventListener);
    return () =>
      window.removeEventListener(
        "vcad:snap-view",
        handleSnapView as EventListener,
      );
  }, []);

  // Fit-sketch view: position the camera perpendicular to the sketch plane at
  // a distance that frames the sketch's U/V bounds in the current viewport.
  const viewportAspect = useThree((s) => s.size.width / Math.max(1, s.size.height));
  useEffect(() => {
    const handleFitSketch = (
      e: CustomEvent<{
        // Kernel Z-up space (caller converts from sketch plane basis)
        planeNormal: { x: number; y: number; z: number };
        planeCenter: { x: number; y: number; z: number };
        // Sketch-local bounds width (U) and height (V) in mm
        width: number;
        height: number;
      }>,
    ) => {
      const { planeNormal, planeCenter, width, height } = e.detail;

      // Kernel Z-up → display Y-up: (x, y, z) → (x, z, -y)
      const [cx, cy, cz] = kernelToDisplay([
        planeCenter.x,
        planeCenter.y,
        planeCenter.z,
      ]);
      const [nx, ny, nz] = kernelToDisplay([
        planeNormal.x,
        planeNormal.y,
        planeNormal.z,
      ]);
      // Re-normalize to guard against any accumulated drift in the source basis.
      const nLen = Math.hypot(nx, ny, nz) || 1;
      const wNormal = new Vector3(nx / nLen, ny / nLen, nz / nLen);

      // Fit distance for a Three.js perspective camera (vertical FOV). We
      // check both the vertical and horizontal extent and pick whichever is
      // tight; a 20% padding keeps the sketch off the viewport edges.
      const perspective = camera as PerspectiveCamera;
      const fovRad = (perspective.isPerspectiveCamera ? perspective.fov : 50) * (Math.PI / 180);
      const aspect = perspective.isPerspectiveCamera
        ? perspective.aspect
        : viewportAspect;
      const padding = 1.2;
      const safeWidth = Math.max(width, 1);
      const safeHeight = Math.max(height, 1);
      const distV = (safeHeight / 2) / Math.tan(fovRad / 2);
      const distH = (safeWidth / 2) / (aspect * Math.tan(fovRad / 2));
      const viewDistance = Math.max(distV, distH, 10) * padding;

      targetGoalRef.current.set(cx, cy, cz);
      const cameraPos = new Vector3(
        cx + wNormal.x * viewDistance,
        cy + wNormal.y * viewDistance,
        cz + wNormal.z * viewDistance,
      );
      cameraPositionGoalRef.current = cameraPos;
      distanceGoalRef.current = viewDistance;

      computeLevelQuaternion(cameraPos, targetGoalRef.current, goalQuatRef.current);

      if (orbitRef.current) orbitRef.current.enabled = false;
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };

    window.addEventListener(
      "vcad:fit-sketch",
      handleFitSketch as EventListener,
    );
    return () =>
      window.removeEventListener(
        "vcad:fit-sketch",
        handleFitSketch as EventListener,
      );
  }, [camera, viewportAspect]);

  // Offscreen render from the AI's camera goal. The AI's `screenshot_viewport`
  // tool calls `window.__vcadCaptureAiCamera(goal)` to grab a frame from the
  // AI's point of view WITHOUT disturbing the user's OrbitControls state.
  //
  // We render the existing scene graph (which already has the kernel Z-up →
  // display Y-up rotation applied by the geometry group) with a temporary
  // camera positioned per the goal, into a WebGLRenderTarget, then read back
  // pixels and Y-flip them into a 2D canvas the caller can encode as JPEG.
  const gl = useThree((s) => s.gl);
  const r3fScene = useThree((s) => s.scene);
  const viewportSize = useThree((s) => s.size);
  useEffect(() => {
    const capture = (goal: CameraGoal): HTMLCanvasElement | null => {
      const w = Math.max(1, viewportSize.width);
      const h = Math.max(1, viewportSize.height);
      const [px, py, pz] = kernelToDisplay(goal.position);
      const [tx, ty, tz] = kernelToDisplay(goal.target);

      const tempCam = new PerspectiveCamera(50, w / h, 0.1, 10000);
      tempCam.up.set(0, 1, 0);
      tempCam.position.set(px, py, pz);
      tempCam.lookAt(tx, ty, tz);
      tempCam.updateMatrixWorld();

      const rt = new WebGLRenderTarget(w, h, {
        samples: 4,
        colorSpace: SRGBColorSpace,
      });

      // EffectComposer/postprocessing passes mutate renderer state (tone
      // mapping, output colorspace) while compositing. If the capture lands
      // between those passes, the scene is rendered linearly but the 2D canvas
      // interprets the bytes as sRGB, producing a green-tinted image. Pin the
      // relevant state to known values here and restore after.
      const prevTarget = gl.getRenderTarget();
      const prevToneMapping = gl.toneMapping;
      const prevToneMappingExposure = gl.toneMappingExposure;
      const prevOutputColorSpace = gl.outputColorSpace;
      gl.toneMapping = ACESFilmicToneMapping;
      gl.toneMappingExposure = 1.0;
      gl.outputColorSpace = SRGBColorSpace;
      gl.setRenderTarget(rt);
      gl.clear();
      gl.render(r3fScene, tempCam);
      gl.setRenderTarget(prevTarget);
      gl.toneMapping = prevToneMapping;
      gl.toneMappingExposure = prevToneMappingExposure;
      gl.outputColorSpace = prevOutputColorSpace;

      const buffer = new Uint8Array(w * h * 4);
      gl.readRenderTargetPixels(rt, 0, 0, w, h, buffer);
      rt.dispose();

      const out = document.createElement("canvas");
      out.width = w;
      out.height = h;
      const ctx = out.getContext("2d");
      if (!ctx) return null;
      const imgData = ctx.createImageData(w, h);
      // WebGL origin is bottom-left; canvas is top-left. Flip rows as we copy.
      const rowBytes = w * 4;
      for (let y = 0; y < h; y++) {
        const src = (h - 1 - y) * rowBytes;
        imgData.data.set(buffer.subarray(src, src + rowBytes), y * rowBytes);
      }
      ctx.putImageData(imgData, 0, 0);
      return out;
    };

    const win = window as unknown as {
      __vcadCaptureAiCamera?: (goal: CameraGoal) => HTMLCanvasElement | null;
    };
    win.__vcadCaptureAiCamera = capture;
    return () => {
      if (win.__vcadCaptureAiCamera === capture) {
        delete win.__vcadCaptureAiCamera;
      }
    };
  }, [gl, r3fScene, viewportSize.width, viewportSize.height]);

  // Hero view: special "Make It Real" presentation angle
  useEffect(() => {
    const handleHeroView = () => {
      // Hero angle: 45deg azimuth, 30deg elevation - dramatic presentation angle
      const heroPos = new Vector3(60, -60, 45);
      const targetVec = new Vector3(0, 0, 0);

      // Animate to hero position
      targetGoalRef.current.copy(targetVec);
      cameraPositionGoalRef.current = heroPos;
      distanceGoalRef.current = null;

      // Compute goal quaternion for smooth orientation interpolation (zero roll)
      computeLevelQuaternion(heroPos, targetVec, goalQuatRef.current);

      // Disable OrbitControls during animation
      if (orbitRef.current) orbitRef.current.enabled = false;
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };

    window.addEventListener("vcad:hero-view", handleHeroView);
    return () => window.removeEventListener("vcad:hero-view", handleHeroView);
  }, []);

  // Pan camera target to a kernel-space point. Triggered by the StatusBar
  // cursor-coord chip when the user types a value and presses Enter. Keeps
  // the camera position and zoom; only the look-at target moves.
  useEffect(() => {
    const handler = (
      e: CustomEvent<{ x: number; y: number; z: number }>,
    ) => {
      const { x, y, z } = e.detail;
      const [dx, dy, dz] = kernelToDisplay([x, y, z]);
      targetGoalRef.current.set(dx, dy, dz);
      cameraPositionGoalRef.current = null;
      distanceGoalRef.current = null;
      if (orbitRef.current) orbitRef.current.enabled = false;
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };
    window.addEventListener("vcad:focus-point", handler as EventListener);
    return () =>
      window.removeEventListener("vcad:focus-point", handler as EventListener);
  }, []);

  // Animated camera-fit. Boot's auto-fit takes the snap path inside
  // useCameraControls so the user doesn't watch a sweep on every reload;
  // this listener handles the user-driven dispatches (View → Fit menu,
  // double-click on empty canvas) and flies the camera into place via the
  // standard target+position+quaternion lerp.
  useEffect(() => {
    const handler = () => {
      const scene = useEngineStore.getState().scene;
      if (!scene || scene.parts.length === 0) return;

      const box = new Box3();
      const tmp = new Vector3();
      let hasPoints = false;
      for (const p of scene.parts) {
        const pos = p.mesh.positions;
        for (let i = 0; i < pos.length; i += 3) {
          tmp.set(pos[i] ?? 0, pos[i + 1] ?? 0, pos[i + 2] ?? 0);
          box.expandByPoint(tmp);
          hasPoints = true;
        }
      }
      if (!hasPoints) return;

      const kernelCenter = new Vector3();
      box.getCenter(kernelCenter);
      const size = new Vector3();
      box.getSize(size);
      const dist = Math.max(size.x, size.y, size.z, 50) * 2;
      // Kernel (Z-up) → display (Y-up).
      const displayCenter = new Vector3(
        kernelCenter.x,
        kernelCenter.z,
        -kernelCenter.y,
      );

      // Preserve the current view direction; fall back to isometric when
      // camera and target coincide (shouldn't happen, but defensive).
      const currentTarget =
        orbitRef.current?.target ?? new Vector3();
      const dir = new Vector3().copy(camera.position).sub(currentTarget);
      if (dir.lengthSq() < 1e-6) dir.set(1, 1, 1);
      dir.normalize();

      const cameraPos = new Vector3()
        .copy(displayCenter)
        .addScaledVector(dir, dist);

      targetGoalRef.current.copy(displayCenter);
      cameraPositionGoalRef.current = cameraPos;
      distanceGoalRef.current = dist;
      computeLevelQuaternion(cameraPos, displayCenter, goalQuatRef.current);
      if (orbitRef.current) orbitRef.current.enabled = false;
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };
    window.addEventListener("vcad:camera-fit", handler);
    return () => window.removeEventListener("vcad:camera-fit", handler);
  }, [camera]);

  // Get effective scene settings
  const sceneSettings = useMemo(
    () => getEffectiveSceneSettings(docScene, isDark),
    [docScene, isDark]
  );

  // Get environment preset name for drei (null = no environment map)
  const environmentPreset = useMemo(() => {
    const env = sceneSettings.environment;
    if (env.type === "None") return null;
    if (env.type === "Preset") {
      return ENVIRONMENT_PRESET_MAP[env.preset] ?? "studio";
    }
    return "studio"; // Custom environments not yet supported by drei
  }, [sceneSettings.environment]);

  const environmentIntensity = useMemo(() => {
    const env = sceneSettings.environment;
    if (env.type === "None") return 0;
    // Default 1.0 gives a full-strength IBL out of the box — metals look
    // like metals (Fusion / Onshape studio look). Docs that need a calmer
    // scene override with env.intensity.
    return env.intensity ?? 1.0;
  }, [sceneSettings.environment]);


  return (
    // KILL-SWITCH: `<Selection>` from @react-three/postprocessing wrapped
    // this subtree to feed the `<Outline>` post-effect's silhouette pass.
    // Even with the bail-out in `<SilhouetteTarget>` (see PR #204), a
    // residual React #185 ("Maximum update depth exceeded") still fires
    // on every load — so we drop the whole Selection/SilhouetteTarget/
    // Outline path until the looping consumer of `selectionContext` is
    // identified. AO + Vignette stay live; only the silhouette outline
    // is missing.
    <>
      {/* Engine-independent content - renders immediately */}
      {/* Scene lights from document settings */}
      {sceneSettings.lights.map((light) => {
        if (light.enabled === false) return null;
        const color = new Color(light.color[0], light.color[1], light.color[2]);

        if (light.kind.type === "Directional") {
          const position = lightDirectionToPosition(light.kind.direction);
          return light.castShadow ? (
            <directionalLight
              key={light.id}
              position={position}
              intensity={light.intensity}
              color={color}
              castShadow
              shadow-mapSize-width={2048}
              shadow-mapSize-height={2048}
              shadow-camera-far={200}
              shadow-camera-left={-100}
              shadow-camera-right={100}
              shadow-camera-top={100}
              shadow-camera-bottom={-100}
              shadow-bias={-0.0001}
            />
          ) : (
            <directionalLight
              key={light.id}
              position={position}
              intensity={light.intensity}
              color={color}
            />
          );
        }

        if (light.kind.type === "Point") {
          return (
            <pointLight
              key={light.id}
              position={[light.kind.position.x, light.kind.position.y, light.kind.position.z]}
              intensity={light.intensity}
              color={color}
              distance={light.kind.distance}
            />
          );
        }

        if (light.kind.type === "Spot") {
          return (
            <spotLight
              key={light.id}
              position={[light.kind.position.x, light.kind.position.y, light.kind.position.z]}
              intensity={light.intensity}
              color={color}
              angle={light.kind.angle ? (light.kind.angle * Math.PI) / 180 : undefined}
              penumbra={light.kind.penumbra ? light.kind.penumbra / 90 : undefined}
            />
          );
        }

        return null;
      })}

      {/* Contact shadows - soft shadow beneath objects (disabled during camera motion for FPS) */}
      {!isPcbMode && !isCameraMoving && (
        <ContactShadows
          position={[0, -0.01, 0]}
          opacity={isDark ? 0.4 : 0.3}
          scale={200}
          blur={2}
          far={100}
          resolution={512}
          color={isDark ? "#000000" : "#1a1a1a"}
        />
      )}

      {/* Grid (3D mode only — PCB mode has its own grid) */}
      {!isPcbMode && <GridPlane />}

      {/* Controls - mouse buttons configured by control scheme.
          Disabled while a WebXR session is active — the headset drives the
          camera and orbit gestures would fight head-tracking. */}
      <OrbitControls
        ref={orbitRef}
        makeDefault
        enabled={!xrPresenting}
        enableDamping={false}
        // Desktop uses a custom wheel zoom handler; touch devices need the
        // built-in pinch-to-dolly path, so enable zoom when the pointer is coarse.
        enableZoom={isCoarsePointer}
        touches={{ ONE: TOUCH.ROTATE, TWO: TOUCH.DOLLY_PAN }}
        mouseButtons={(() => {
          const schemeButtons = getOrbitControlsMouseButtons(
            controlScheme,
            effectiveDevice,
          );
          return {
            LEFT: undefined, // LMB reserved for selection
            MIDDLE: schemeButtons.MIDDLE,
            RIGHT: schemeButtons.RIGHT,
          };
        })()}
      />

      {/* Orientation gizmo - RGB axes, click to snap view (3D mode only).
          Hidden in XR — it's a 2D screen-space overlay that doesn't make
          sense in stereo. */}
      {!isPcbMode && !xrPresenting && (
        <GizmoHelper alignment="bottom-right" margin={[80, 80]}>
          <GizmoViewport
            axisColors={["#e06c75", "#61afef", "#98c379"]}
            labelColor="#abb2bf"
            labels={["X", "Z", "Y"]}
          />
        </GizmoHelper>
      )}

      {/* Environment lighting (3D mode only) */}
      {!isPcbMode && environmentPreset && (
        <Suspense fallback={null}>
          <Environment
            preset={environmentPreset as "studio" | "warehouse" | "apartment" | "park" | "city" | "dawn" | "night" | "sunset" | "forest"}
            environmentIntensity={environmentIntensity}
            background={sceneSettings.background.type === "Environment"}
          />
        </Suspense>
      )}

      {/* Default procedural studio IBL — three-point rig baked into a
          PMREM cubemap. Reads as a photography studio: a cool-white key
          softbox above, warmer fill panels, and a brighter rim panel
          behind the camera. Colored subtly rather than pure white so
          metallic reflections pick up the temperature shift the way they
          do in a real product shot. Reflections are modulated down at
          the ground to avoid a flat mirrored floor look.

          Resolution stays at 256 because PMREMGenerator runs the
          prefilter synchronously on the main thread; 512 quadruples
          that cost and froze the render loop for ~10s on boot. 256 is
          still plenty for a smooth prefiltered reflection because
          roughness 0.1+ reads from higher mips anyway. Only active
          when the user hasn't selected an explicit environment preset. */}
      {!isPcbMode && !environmentPreset && (
        <Environment resolution={256} frames={1} background={false}>
          {/* Backdrop: near-black in dark theme, warm studio wall in
              light theme. Kept as a flat color (not a shader gradient)
              so PMREMGenerator doesn't hit a custom-shader compile on
              boot — that was the source of the ~10s first-paint stall. */}
          <color
            attach="background"
            args={isDark ? [0.018, 0.02, 0.024] : [0.62, 0.6, 0.58]}
          />

          {/* Key light — cool-white ceiling softbox, tilted forward so
              it reads as "camera-left overhead" rather than a flat
              ceiling. This is the dominant highlight on the top of
              parts. */}
          <Lightformer
            form="rect"
            intensity={isDark ? 3.2 : 5.5}
            color={[1.0, 0.99, 0.97]}
            position={[3, 9, 3]}
            rotation={[-Math.PI / 2 + 0.25, 0, 0.2]}
            scale={[18, 14, 1]}
          />

          {/* Fill — warmer, camera-right, lower. Fills shadows with a
              slight amber cast, the way a bounce card warms up the
              shadow side of a product shot. */}
          <Lightformer
            form="rect"
            intensity={isDark ? 1.3 : 2.2}
            color={[1.0, 0.94, 0.86]}
            position={[10, 2, 4]}
            rotation={[0, -Math.PI / 2, 0.1]}
            scale={[12, 8, 1]}
          />

          {/* Opposite-side fill — slightly cooler, keeps the left side
              from going too dark. */}
          <Lightformer
            form="rect"
            intensity={isDark ? 0.9 : 1.6}
            color={[0.96, 0.98, 1.0]}
            position={[-10, 2, 0]}
            rotation={[0, Math.PI / 2, -0.1]}
            scale={[12, 8, 1]}
          />

          {/* Rim — bright narrow strip behind the camera to carve the
              silhouette on metallic edges. This is the source of the
              sharp highlight line on the far side of polished parts. */}
          <Lightformer
            form="rect"
            intensity={isDark ? 2.2 : 3.6}
            color={[0.98, 1.0, 1.0]}
            position={[0, 4, -12]}
            rotation={[0, 0, 0]}
            scale={[14, 5, 1]}
          />

          {/* Faint floor bounce — keeps undersides from reading
              completely black on metals. Dim by design. */}
          <Lightformer
            form="rect"
            intensity={isDark ? 0.25 : 0.5}
            color={[0.92, 0.94, 0.96]}
            position={[0, -8, 0]}
            rotation={[Math.PI / 2, 0, 0]}
            scale={[20, 20, 1]}
          />
        </Environment>
      )}

      {/* Background: match UI chrome so the viewport blends with the app.
          The <Environment> above still produces IBL via scene.environment,
          so metallic reflections keep their studio highlights. */}
      {!isPcbMode && !docScene?.background && (
        <color attach="background" args={[isDark ? BG_DARK : BG_LIGHT]} />
      )}
      {!isPcbMode && docScene?.background?.type === "Solid" && (
        <color attach="background" args={[docScene.background.color[0], docScene.background.color[1], docScene.background.color[2]]} />
      )}
      {!isPcbMode && docScene?.background?.type === "Transparent" && (
        <color attach="background" args={[0, 0, 0]} />
      )}

      {/* ═══════════════════════════════════════════════════════════════════
          PCB MODE: Render PcbScene inside the rotation group
          ═══════════════════════════════════════════════════════════════════ */}
      {isPcbMode && (
        <group rotation={[-Math.PI / 2, 0, 0]}>
          <PcbScene />
        </group>
      )}

      {/* ═══════════════════════════════════════════════════════════════════
          3D MODE: Render standard CAD scene content
          ═══════════════════════════════════════════════════════════════════ */}
      {!isPcbMode && engineReady && (
        <>
          {/* Ray-traced viewport sync (camera state for overlay) */}
          {renderMode === "raytrace" && raytraceAvailable && (
            <RayTracedViewportSync />
          )}

          {/* Scene meshes - always render (ray trace overlays on top for BRep parts) */}
          {/* Wrap all kernel geometry in Z-up → Y-up rotation so the kernel
              stays in standard CAD Z-up while Three.js renders Y-up.
              `XRSceneTransform` rescales + repositions to desktop-scale while
              a WebXR session is active; outside XR it is a passthrough. */}
          <XRSceneTransform>
          <group rotation={[-Math.PI / 2, 0, 0]}>
            {/* Plane gizmo at origin - inside rotation group so kernel planes display correctly */}
            <PlaneGizmo />

            {/* KILL-SWITCH: was `<SilhouetteTarget enabled={silhouetteEnabled}>`
                feeding the Outline post-effect via Selection context. Bypassing
                the whole subtree until React #185 root cause is found. */}
            <>
              {/* Scene meshes - Assembly mode (instances) */}
              {scene?.instances?.map((inst: EvaluatedInstance) => {
                const instanceSelectionId = getInstanceSelectionId(inst);
                // Create a minimal PartInfo-like object for instance rendering
                const instancePartInfo: PartInfo = {
                  id: instanceSelectionId,
                  name: inst.name ?? inst.partDefId,
                  kind: "cube", // Placeholder kind for instances
                  primitiveNodeId: 0,
                  scaleNodeId: 0,
                  rotateNodeId: 0,
                  translateNodeId: 0,
                };
                return (
                  <SceneMesh
                    key={inst.instanceId}
                    partInfo={instancePartInfo}
                    mesh={inst.mesh}
                    materialKey={inst.material}
                    selected={selectedPartIds.has(instanceSelectionId)}
                    transform={inst.transform}
                  />
                );
              })}

              {/* Imported meshes (no PartInfo - direct mesh display) */}
              {(!scene?.instances || scene.instances.length === 0) &&
                parts.length === 0 &&
                scene?.parts.map((evalPart, idx) => (
                  <ImportedMesh
                    key={`imported-${idx}`}
                    mesh={evalPart.mesh}
                    materialKey={evalPart.material}
                  />
                ))}

              {/* Scene meshes - Legacy mode (parts with PartInfo) */}
              {(!scene?.instances || scene.instances.length === 0) &&
                parts.length > 0 &&
                scene?.parts.map((evalPart, idx) => {
                  const partInfo = parts[idx];
                  if (!partInfo) return null;
                  return (
                    <SceneMesh
                      key={partInfo.id}
                      partInfo={partInfo}
                      mesh={evalPart.mesh}
                      materialKey={evalPart.material}
                      selected={isPartSelected(partInfo.id, idx)}
                    />
                  );
                })}
            </>

              {/* Debug: mesh boundary edges (holes in tessellation).
                  Toggle with Ctrl+Shift+B or
                  __VCAD_DEBUG_OVERLAY.getState().toggleBoundaryEdges() */}
              <DebugBoundaryOverlays />
              {/* Debug: click-to-inspect triangle picker.
                  Toggle with Ctrl+Shift+T. */}
              <DebugTriangleInspector />
          {/* Clash visualization (zebra pattern on intersections) */}
          {scene?.clashes.map((clashMesh, idx) => (
            <ClashMesh key={`clash-${idx}`} mesh={clashMesh} />
          ))}

          {/* Extrusion preview (semi-transparent) */}
          {previewMesh && <PreviewMesh mesh={previewMesh} />}

          {/* 3D Sketch plane (when sketch mode is active) */}
          {sketchActive && <SketchPlane3D />}

          {/* Selection bounding box overlay */}
          <SelectionOverlay />

          {/* Dimension annotations for primitives */}
          <DimensionOverlay />

          {/* Other participants' camera frustums (AI for now, peers later) */}
          <ParticipantCameraOverlay />

          {/* DFM (Design for Manufacturing) issue badges. No-op when the
              DFM panel hasn't been enabled. */}
          <DfmAnnotations />

          {/* XR presence — broadcasts local headset/hand pose and renders
              remote peers. No-op outside XR / outside cloud-sync. Inside the
              rotation+scale group so its avatars are in scene-local space. */}
          <XRPresence />
          </group>
          </XRSceneTransform>

          {/* XR gesture interpreter — only does work while a WebXR session
              is presenting. Outside the scene transform so its raycasts run
              in unscaled world space. */}
          <XRGestures />

          {/* Transform gizmo — outside rotation group; does its own Z-up ↔ Y-up conversion
              so that gizmo handle colors (RGB=XYZ) match the kernel axis colors */}
          <TransformGizmo orbitControls={orbitRef} />
        </>
      )}

      {/* Post-processing effects. Sample counts drop while the camera is
          moving so the scene keeps depth without tanking framerate, then
          ramp back up once orbit settles. Disabled entirely while a WebXR
          session is active — EffectComposer renders to an offscreen target
          and blits to the canvas, which doesn't write to the XR layer's
          framebuffer, so in VR/AR the scene would go black and only
          objects rendered directly by WebXRManager (hands, controllers)
          would show. */}
      {engineReady && !xrPresenting && (() => {
        const aoEnabled = sceneSettings.postProcessing.ambientOcclusion?.enabled !== false;
        const vignetteEnabled = sceneSettings.postProcessing.vignette?.enabled !== false;
        if (!aoEnabled && !vignetteEnabled) return null;
        // EffectComposer's children type is strict (`JSX.Element | JSX.Element[]`),
        // so we build the array up-front rather than inlining `cond && <Effect/>`
        // expressions, which would resolve to `false` when disabled.
        const effects: React.JSX.Element[] = [];
        if (aoEnabled) {
          effects.push(
            <N8AO
              key="ao"
              aoRadius={sceneSettings.postProcessing.ambientOcclusion?.radius ?? 0.5}
              intensity={sceneSettings.postProcessing.ambientOcclusion?.intensity ?? (isDark ? 2 : 1.5)}
              aoSamples={isCameraMoving ? 3 : 6}
              denoiseSamples={isCameraMoving ? 1 : 4}
            />,
          );
        }
        // KILL-SWITCH: `<Outline>` requires the `<Selection>` provider, which
        // we removed above. Skip it.
        if (vignetteEnabled) {
          effects.push(
            <Vignette
              key="vignette"
              offset={sceneSettings.postProcessing.vignette?.offset ?? 0.3}
              darkness={sceneSettings.postProcessing.vignette?.darkness ?? (isDark ? 0.5 : 0.3)}
              eskil={false}
            />,
          );
        }
        return <EffectComposer>{effects}</EffectComposer>;
      })()}
    </>
  );
}
