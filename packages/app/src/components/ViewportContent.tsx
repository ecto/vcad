import { useRef, useEffect, useMemo, useState, Suspense } from "react";
import { Spherical, Vector3, Box3, Raycaster, Vector2, Quaternion, Matrix4, Color, TOUCH } from "three";

const isCoarsePointer =
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(pointer: coarse)").matches;
import { useThree, useFrame } from "@react-three/fiber";
import {
  OrbitControls,
  GizmoHelper,
  GizmoViewport,
  Environment,
  ContactShadows,
  Html,
} from "@react-three/drei";
import { EffectComposer, N8AO, Vignette } from "@react-three/postprocessing";
import type { OrbitControls as OrbitControlsImpl } from "three-stdlib";
import { GridPlane } from "./GridPlane";
import { SceneMesh, ImportedMesh } from "./SceneMesh";
import { ClashMesh } from "./ClashMesh";
import { PreviewMesh } from "./PreviewMesh";
import { SketchPlane3D } from "./SketchPlane3D";
import { PlaneGizmo } from "./PlaneGizmo";
import { TransformGizmo } from "./TransformGizmo";
import { SelectionOverlay } from "./SelectionOverlay";
import { DimensionOverlay } from "./DimensionOverlay";
import { RayTracedViewportSync } from "./RayTracedViewport";
import {
  useEngineStore,
  useDocumentStore,
  useUiStore,
  useSketchStore,
} from "@vcad/core";
import type { PartInfo } from "@vcad/core";
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
  neutral: "studio", // fallback for neutral
};

// Effective scene settings type (all fields required)
interface EffectiveSceneSettings {
  environment: NonNullable<SceneSettings["environment"]>;
  lights: IrLight[];
  background: NonNullable<SceneSettings["background"]>;
  postProcessing: NonNullable<SceneSettings["postProcessing"]>;
}

// Default scene settings (smart defaults)
const DEFAULT_SCENE_SETTINGS: EffectiveSceneSettings = {
  environment: { type: "None" },
  lights: [
    {
      id: "key",
      kind: { type: "Directional", direction: { x: 0.5, y: -0.8, z: 0.4 } },
      color: [1, 0.98, 0.95],
      intensity: 1.2,
      castShadow: true,
    },
    {
      id: "fill",
      kind: { type: "Directional", direction: { x: -0.3, y: -0.4, z: -0.2 } },
      color: [0.95, 0.97, 1.0],
      intensity: 0.4,
    },
    {
      id: "rim",
      kind: { type: "Directional", direction: { x: -0.5, y: -0.2, z: 0.5 } },
      color: [1, 1, 1],
      intensity: 0.2,
    },
  ],
  background: { type: "Environment" },
  postProcessing: {
    ambientOcclusion: { enabled: true, intensity: 1.5, radius: 0.5 },
    vignette: { enabled: true, offset: 0.3, darkness: 0.3 },
  },
};

// Compute effective scene settings (merge document settings with defaults)
function getEffectiveSceneSettings(scene: SceneSettings | undefined, isDark: boolean): EffectiveSceneSettings {
  const base = DEFAULT_SCENE_SETTINGS;

  // Adjust defaults for dark mode
  const darkModePostProcessing = isDark ? {
    ambientOcclusion: { enabled: true, intensity: 2, radius: 0.5 },
    vignette: { enabled: true, offset: 0.3, darkness: 0.5 },
  } : base.postProcessing;

  if (!scene) {
    return { ...base, postProcessing: darkModePostProcessing };
  }

  return {
    environment: scene.environment ?? base.environment,
    lights: scene.lights ?? base.lights,
    background: scene.background ?? base.background,
    postProcessing: scene.postProcessing ?? darkModePostProcessing,
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
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const setOrbiting = useUiStore((s) => s.setOrbiting);
  const renderMode = useUiStore((s) => s.renderMode);
  const raytraceAvailable = useUiStore((s) => s.raytraceAvailable);
  const sketchActive = useSketchStore((s) => s.active);
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

  // Initial camera state for reset
  const INITIAL_POSITION = new Vector3(50, 50, 50);
  const INITIAL_TARGET = new Vector3(0, 0, 0);
  const INITIAL_DISTANCE = INITIAL_POSITION.distanceTo(INITIAL_TARGET);

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
  const isPartSelected = (partId: string, partIndex: number): boolean => {
    // Direct part ID match
    if (selectedPartIds.has(partId)) return true;
    // Instance ID match (for assembly mode)
    const instanceId = rootIndexToInstanceId.get(partIndex);
    if (instanceId && selectedPartIds.has(instanceId)) return true;
    return false;
  };

  // Calculate center and size of selected parts/instances
  const selectionInfo = useMemo(() => {
    if (selectedPartIds.size === 0 || !scene) return null;

    const box = new Box3();
    const tempVec = new Vector3();
    let hasPoints = false;

    // Assembly mode: check instances
    if (scene.instances && scene.instances.length > 0) {
      for (const inst of scene.instances) {
        const instanceSelectionId = getInstanceSelectionId(inst);
        if (!instanceSelectionId || !selectedPartIds.has(instanceSelectionId))
          continue;

        const positions = inst.mesh.positions;
        const t = inst.transform ?? {
          translation: { x: 0, y: 0, z: 0 },
          rotation: { x: 0, y: 0, z: 0 },
          scale: { x: 1, y: 1, z: 1 },
        };
        for (let i = 0; i < positions.length; i += 3) {
          // Apply instance transform to positions for accurate bounding box
          tempVec.set(
            positions[i]! * t.scale.x + t.translation.x,
            positions[i + 1]! * t.scale.y + t.translation.y,
            positions[i + 2]! * t.scale.z + t.translation.z,
          );
          box.expandByPoint(tempVec);
          hasPoints = true;
        }
      }
    } else {
      // Legacy mode: check parts (also handles instance IDs via isPartSelected)
      parts.forEach((part, index) => {
        if (!isPartSelected(part.id, index)) return;
        const evalPart = scene.parts[index];
        if (!evalPart) return;

        const positions = evalPart.mesh.positions;
        for (let i = 0; i < positions.length; i += 3) {
          tempVec.set(positions[i]!, positions[i + 1]!, positions[i + 2]!);
          box.expandByPoint(tempVec);
          hasPoints = true;
        }
      });
    }

    if (!hasPoints) return null;
    const kernelCenter = new Vector3();
    box.getCenter(kernelCenter);
    const size = new Vector3();
    box.getSize(size);
    const maxDim = Math.max(size.x, size.y, size.z);
    // Transform kernel Z-up center to Three.js Y-up world space
    // Rotation -90° around X: (x, y, z) → (x, z, -y)
    const center = new Vector3(kernelCenter.x, kernelCenter.z, -kernelCenter.y);
    return { center, maxDim };
  }, [selectedPartIds, scene, parts, rootIndexToInstanceId]);

  // Animate orbit target to selection center and zoom to fit
  // Skip during gizmo drag to avoid fighting with the user's transform
  useEffect(() => {
    if (selectionInfo && !isDraggingGizmo) {
      targetGoalRef.current.copy(selectionInfo.center);
      // Distance = 2.5x the max dimension, clamped to reasonable range
      distanceGoalRef.current = Math.max(
        30,
        Math.min(300, selectionInfo.maxDim * 2.5),
      );
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    }
  }, [selectionInfo, isDraggingGizmo]);

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
  }, [camera, controlScheme, effectiveDevice, zoomBehavior, orbitMomentum]);

  // Disable raycasting and expensive effects during orbit for performance
  useEffect(() => {
    const controls = orbitRef.current;
    if (!controls) return;

    const handleStart = () => {
      setOrbiting(true);
      setIsCameraMoving(true);
    };
    const handleEnd = () => {
      setOrbiting(false);
      setIsCameraMoving(false);
    };

    controls.addEventListener("start", handleStart);
    controls.addEventListener("end", handleEnd);

    return () => {
      controls.removeEventListener("start", handleStart);
      controls.removeEventListener("end", handleEnd);
    };
  }, [setOrbiting]);

  // Double-click on empty canvas resets camera to initial position
  useEffect(() => {
    const controls = orbitRef.current;
    const domElement = controls?.domElement;
    if (!domElement) return;

    const handleDoubleClick = () => {
      // Only reset when nothing is selected
      if (useUiStore.getState().selectedPartIds.size > 0) return;

      // Animate to initial position
      targetGoalRef.current.copy(INITIAL_TARGET);
      distanceGoalRef.current = INITIAL_DISTANCE;
      cameraPositionGoalRef.current = null; // Clear any position goal
      isAnimatingTargetRef.current = true;
      setIsCameraMoving(true);
    };

    domElement.addEventListener("dblclick", handleDoubleClick);
    return () => domElement.removeEventListener("dblclick", handleDoubleClick);
  }, []);

  // Face selection: swing camera to view face flat
  useEffect(() => {
    const handleFaceSelected = (
      e: CustomEvent<{
        normal: { x: number; y: number; z: number };
        centroid: { x: number; y: number; z: number };
      }>,
    ) => {
      const { normal, centroid } = e.detail;

      // centroid is in world space (from Three.js e.point), but normal is in
      // kernel Z-up space — transform normal: (nx, ny, nz) → (nx, nz, -ny)
      const wNormal = new Vector3(normal.x, normal.z, -normal.y);

      // Set target to face centroid (already world space)
      targetGoalRef.current.set(centroid.x, centroid.y, centroid.z);

      // Camera should be positioned along the positive normal (in front of the face, looking at it)
      // Distance of 60mm for a good view
      const viewDistance = 60;
      const cameraPos = new Vector3(
        centroid.x + wNormal.x * viewDistance,
        centroid.y + wNormal.y * viewDistance,
        centroid.z + wNormal.z * viewDistance,
      );
      cameraPositionGoalRef.current = cameraPos;
      distanceGoalRef.current = viewDistance;

      // Compute goal quaternion for smooth orientation interpolation (zero roll)
      const targetVec = new Vector3(centroid.x, centroid.y, centroid.z);
      computeLevelQuaternion(cameraPos, targetVec, goalQuatRef.current);

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
  }, []);

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
    return env.intensity ?? 0.4;
  }, [sceneSettings.environment]);

  return (
    <>
      {/* Engine-independent content - renders immediately */}
      {/* Ambient light when no environment map provides indirect illumination */}
      {!environmentPreset && <ambientLight intensity={0.4} />}
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

      {/* Controls - mouse buttons configured by control scheme */}
      <OrbitControls
        ref={orbitRef}
        makeDefault
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

      {/* Orientation gizmo - RGB axes, click to snap view (3D mode only) */}
      {!isPcbMode && (
        <GizmoHelper alignment="bottom-right" margin={[80, 80]}>
          <GizmoViewport
            axisColors={["#e06c75", "#61afef", "#98c379"]}
            labelColor="#abb2bf"
            labels={["X", "Z", "Y"]}
          />
        </GizmoHelper>
      )}

      {/* Subtle loading indicator while engine initializes (3D mode) */}
      {!isPcbMode && !engineReady && (
        <Html position={[0, 0, 0]} center>
          <div className="text-xs text-text-muted opacity-50">
            loading engine...
          </div>
        </Html>
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

      {/* Custom background (if not using environment) */}
      {!isPcbMode && sceneSettings.background.type === "Solid" && (
        <color attach="background" args={[sceneSettings.background.color[0], sceneSettings.background.color[1], sceneSettings.background.color[2]]} />
      )}
      {!isPcbMode && sceneSettings.background.type === "Transparent" && (
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
              stays in standard CAD Z-up while Three.js renders Y-up */}
          <group rotation={[-Math.PI / 2, 0, 0]}>
            {/* Plane gizmo at origin - inside rotation group so kernel planes display correctly */}
            <PlaneGizmo />

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
          </group>

          {/* Transform gizmo — outside rotation group; does its own Z-up ↔ Y-up conversion
              so that gizmo handle colors (RGB=XYZ) match the kernel axis colors */}
          <TransformGizmo orbitControls={orbitRef} />
        </>
      )}

      {/* Post-processing effects - disabled during camera motion for FPS */}
      {engineReady && !isCameraMoving && sceneSettings.postProcessing.ambientOcclusion?.enabled !== false && sceneSettings.postProcessing.vignette?.enabled !== false && (
        <EffectComposer>
          <N8AO
            aoRadius={sceneSettings.postProcessing.ambientOcclusion?.radius ?? 0.5}
            intensity={sceneSettings.postProcessing.ambientOcclusion?.intensity ?? (isDark ? 2 : 1.5)}
            aoSamples={6}
            denoiseSamples={4}
          />
          <Vignette
            offset={sceneSettings.postProcessing.vignette?.offset ?? 0.3}
            darkness={sceneSettings.postProcessing.vignette?.darkness ?? (isDark ? 0.5 : 0.3)}
            eskil={false}
          />
        </EffectComposer>
      )}
      {/* AO only mode */}
      {engineReady && !isCameraMoving && sceneSettings.postProcessing.ambientOcclusion?.enabled !== false && sceneSettings.postProcessing.vignette?.enabled === false && (
        <EffectComposer>
          <N8AO
            aoRadius={sceneSettings.postProcessing.ambientOcclusion?.radius ?? 0.5}
            intensity={sceneSettings.postProcessing.ambientOcclusion?.intensity ?? (isDark ? 2 : 1.5)}
            aoSamples={6}
            denoiseSamples={4}
          />
        </EffectComposer>
      )}
      {/* Vignette only mode */}
      {engineReady && !isCameraMoving && sceneSettings.postProcessing.ambientOcclusion?.enabled === false && sceneSettings.postProcessing.vignette?.enabled !== false && (
        <EffectComposer>
          <Vignette
            offset={sceneSettings.postProcessing.vignette?.offset ?? 0.3}
            darkness={sceneSettings.postProcessing.vignette?.darkness ?? (isDark ? 0.5 : 0.3)}
            eskil={false}
          />
        </EffectComposer>
      )}
    </>
  );
}
