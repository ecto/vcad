import { useFrame, useThree } from "@react-three/fiber";
import { useRef } from "react";
import * as THREE from "three";
import { useDocumentStore } from "@vcad/core";
import { useXRPresenting, useXRSupportStore, type XRViewTransform } from "@/stores/xr-store";

/**
 * WebXR pinch / bimanual gesture interpreter.
 *
 * Reads thumb-tip and index-finger-tip joint poses from the active XRSession
 * each frame, detects pinches, and runs a small state machine that maps two
 * canonical XR-CAD gestures onto existing app affordances:
 *
 *   - **Fillet (M2):** pinch a part with one hand, pinch with the other,
 *     release → commit `addFillet(partId, distanceBetweenHands)`.
 *   - **Scale teleport (M3):** bimanual pinch in empty space → grab the
 *     scene; spreading hands scales it up, drawing them together scales it
 *     down. Translating both hands together pans the scene. Release to
 *     freeze the new view.
 *
 * Outside an XR session this component renders nothing and does no work.
 */

const PINCH_CLOSE = 0.025; // 2.5 cm — index-tip to thumb-tip
const PINCH_OPEN = 0.04; // hysteresis: must open past this to count as released
const REACH_RADIUS = 0.4; // 40 cm reach-ray length for part picking
const MIN_RADIUS_M = 0.005; // 5 mm physical at desktop scale; below this we skip
const MAX_RADIUS_M = 0.5;
/** Convert metres of separation to millimetres of model-space radius. The
 * default XR transform scales the scene by 1/1000, so 1 physical metre
 * between hands = 1000 mm of fillet radius. */
const M_TO_MODEL_MM = 1000;

const SCALE_MIN = 0.0001; // 1 mm scene = 0.1 mm physical (zoomed way out)
const SCALE_MAX = 10.0; // 1 mm scene = 10 mm physical (way zoomed in)

/** Read joint position in world space; returns false if pose unavailable. */
function readJoint(
  frame: XRFrame,
  hand: XRHand,
  jointName: XRHandJoint,
  refSpace: XRReferenceSpace,
  out: THREE.Vector3,
): boolean {
  const joint = hand.get(jointName);
  if (!joint) return false;
  const pose = frame.getJointPose?.(joint, refSpace);
  if (!pose) return false;
  const p = pose.transform.position;
  out.set(p.x, p.y, p.z);
  return true;
}

interface PinchSample {
  pinching: boolean;
  midpoint: THREE.Vector3;
  /** Direction from wrist toward pinch midpoint — used as a "reach ray". */
  reach: THREE.Vector3;
}

const _thumb = new THREE.Vector3();
const _index = new THREE.Vector3();
const _wrist = new THREE.Vector3();

function samplePinch(
  frame: XRFrame,
  hand: XRHand,
  refSpace: XRReferenceSpace,
  prevPinching: boolean,
  out: PinchSample,
): boolean {
  if (
    !readJoint(frame, hand, "thumb-tip", refSpace, _thumb) ||
    !readJoint(frame, hand, "index-finger-tip", refSpace, _index) ||
    !readJoint(frame, hand, "wrist", refSpace, _wrist)
  ) {
    return false;
  }
  const dist = _thumb.distanceTo(_index);
  // Hysteresis so the pinch doesn't chatter at the boundary.
  const pinching = prevPinching ? dist < PINCH_OPEN : dist < PINCH_CLOSE;
  out.pinching = pinching;
  out.midpoint.copy(_thumb).add(_index).multiplyScalar(0.5);
  out.reach.copy(out.midpoint).sub(_wrist).normalize();
  return true;
}

/** Walk the scene graph to find the part ID associated with a hit object. */
function partIdFromHit(obj: THREE.Object3D | null): string | null {
  let cur: THREE.Object3D | null = obj;
  while (cur) {
    const id = (cur.userData as { partId?: unknown } | undefined)?.partId;
    if (typeof id === "string") return id;
    cur = cur.parent;
  }
  return null;
}

const _raycaster = new THREE.Raycaster();
const _rayOrigin = new THREE.Vector3();
const _rayDir = new THREE.Vector3();

/** Cast a short reach-ray from the pinch midpoint toward the part being
 * pointed at. Returns the partId whose mesh is hit, or null. */
function pickPartId(
  scene: THREE.Scene,
  midpoint: THREE.Vector3,
  reach: THREE.Vector3,
): string | null {
  // Start the ray slightly behind the midpoint so a hand inside a mesh still
  // produces a hit.
  _rayOrigin.copy(midpoint).addScaledVector(reach, -0.02);
  _rayDir.copy(reach);
  _raycaster.set(_rayOrigin, _rayDir);
  _raycaster.far = REACH_RADIUS + 0.02;
  const hits = _raycaster.intersectObject(scene, true);
  for (const hit of hits) {
    const id = partIdFromHit(hit.object);
    if (id) return id;
  }
  return null;
}

interface HandState {
  pinching: boolean;
  midpoint: THREE.Vector3;
}

interface FilletCapture {
  kind: "fillet";
  partId: string;
}

interface ScaleCapture {
  kind: "scale";
  /** Midpoint of the two pinch midpoints when the gesture began (world). */
  initialAnchor: THREE.Vector3;
  /** Distance between the two pinches when the gesture began (metres). */
  initialDistance: number;
  /** View transform when the gesture began. */
  initialView: XRViewTransform;
  /** Anchor in scene-local space (i.e. unscaled, untranslated). */
  sceneAnchor: THREE.Vector3;
}

type Capture = FilletCapture | ScaleCapture | null;

const _midA = new THREE.Vector3();
const _midB = new THREE.Vector3();
const _bimid = new THREE.Vector3();
const _newPos = new THREE.Vector3();

export function XRGestures() {
  const presenting = useXRPresenting();
  const { gl, scene } = useThree();

  const leftRef = useRef<HandState>({
    pinching: false,
    midpoint: new THREE.Vector3(),
  });
  const rightRef = useRef<HandState>({
    pinching: false,
    midpoint: new THREE.Vector3(),
  });
  const captureRef = useRef<Capture>(null);
  const sampleRef = useRef<PinchSample>({
    pinching: false,
    midpoint: new THREE.Vector3(),
    reach: new THREE.Vector3(),
  });

  useFrame((_, __, frame) => {
    if (!presenting) return;
    const xrFrame = frame as XRFrame | undefined;
    if (!xrFrame) return;
    const session = gl.xr.getSession();
    const refSpace = gl.xr.getReferenceSpace();
    if (!session || !refSpace) return;

    // --- 1. Sample both hands ----------------------------------------------
    for (const src of session.inputSources) {
      if (!src.hand) continue;
      const handRef = src.handedness === "left" ? leftRef : rightRef;
      const sample = sampleRef.current;
      if (!samplePinch(xrFrame, src.hand, refSpace, handRef.current.pinching, sample)) {
        continue;
      }
      handRef.current.pinching = sample.pinching;
      handRef.current.midpoint.copy(sample.midpoint);

      // --- 2. First-pinch capture: try to grab a part for fillet mode ----
      // Only enter fillet mode if no capture is active and this hand just
      // pinched; the "second hand pinches first" case is handled below as
      // scale-teleport.
      if (sample.pinching && captureRef.current == null) {
        const partId = pickPartId(scene, sample.midpoint, sample.reach);
        if (partId) {
          captureRef.current = { kind: "fillet", partId };
        }
      }
    }

    const left = leftRef.current;
    const right = rightRef.current;
    const bothPinching = left.pinching && right.pinching;

    // --- 3. Scale-teleport entry: bimanual pinch with no part captured. ---
    if (bothPinching && captureRef.current == null) {
      _midA.copy(left.midpoint);
      _midB.copy(right.midpoint);
      const initialDistance = _midA.distanceTo(_midB);
      _bimid.copy(_midA).add(_midB).multiplyScalar(0.5);
      const initialView = useXRSupportStore.getState().view;
      // sceneAnchor = (worldAnchor - viewPosition) / viewScale  — the point
      // in the kernel scene's local frame the user just "grabbed".
      const sceneAnchor = new THREE.Vector3()
        .copy(_bimid)
        .sub(new THREE.Vector3(...initialView.position))
        .divideScalar(initialView.scale);
      captureRef.current = {
        kind: "scale",
        initialAnchor: _bimid.clone(),
        initialDistance,
        initialView,
        sceneAnchor,
      };
    }

    // --- 4. Active scale-teleport: drive the view transform live. ---------
    const capture = captureRef.current;
    if (capture && capture.kind === "scale" && bothPinching) {
      _midA.copy(left.midpoint);
      _midB.copy(right.midpoint);
      const distance = _midA.distanceTo(_midB);
      _bimid.copy(_midA).add(_midB).multiplyScalar(0.5);
      const ratio =
        capture.initialDistance > 1e-6
          ? distance / capture.initialDistance
          : 1;
      let scale = capture.initialView.scale * ratio;
      if (scale < SCALE_MIN) scale = SCALE_MIN;
      if (scale > SCALE_MAX) scale = SCALE_MAX;
      // Keep the grabbed scene point pinned to the current bimanual midpoint:
      // worldPoint = sceneAnchor * scale + position  ⇒  position = bimid − sceneAnchor·scale.
      _newPos
        .copy(capture.sceneAnchor)
        .multiplyScalar(scale)
        .multiplyScalar(-1)
        .add(_bimid);
      useXRSupportStore.getState().setView({
        scale,
        position: [_newPos.x, _newPos.y, _newPos.z],
      });
      return;
    }

    // --- 5. Release handling ----------------------------------------------
    if (!capture) return;

    // Capture remains active until both hands are open — stops the rare
    // single-frame chatter where one hand registers as released a frame
    // before the other.
    const eitherReleased = !left.pinching || !right.pinching;
    if (!eitherReleased) return;

    if (capture.kind === "fillet") {
      const distance = left.midpoint.distanceTo(right.midpoint);
      captureRef.current = null;
      if (distance < MIN_RADIUS_M || distance > MAX_RADIUS_M) return;
      // Convert physical distance to model-space mm. Use the live view scale
      // so a fillet feels the same regardless of how zoomed-in we are: 1 cm
      // hand-spread always reads as 1 cm at the current scale.
      const view = useXRSupportStore.getState().view;
      const scaleFactor = view.scale > 1e-9 ? 0.001 / view.scale : 1;
      const radius = distance * M_TO_MODEL_MM * scaleFactor;
      useDocumentStore.getState().addFillet(capture.partId, radius);
      return;
    }

    if (capture.kind === "scale") {
      // The view transform was already committed live; just exit.
      captureRef.current = null;
      return;
    }
  });

  return null;
}
