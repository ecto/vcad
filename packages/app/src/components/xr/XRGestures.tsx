import { useFrame, useThree } from "@react-three/fiber";
import { useRef } from "react";
import * as THREE from "three";
import { useDocumentStore } from "@vcad/core";
import { useXRPresenting } from "@/stores/xr-store";

/**
 * WebXR pinch / bimanual gesture interpreter.
 *
 * Reads thumb-tip and index-finger-tip joint poses from the active XRSession
 * each frame, detects pinches, and runs a small state machine that maps the
 * canonical CAD-in-XR gesture — bimanual pinch on a part — to the existing
 * `useDocumentStore.addFillet` op:
 *
 *   1. One hand pinches near a part   → that part becomes the fillet target.
 *   2. The other hand pinches anywhere → bimanual capture; the distance
 *      between the two pinch midpoints becomes the live radius candidate.
 *   3. Either hand releases            → if a target was captured, commit
 *      `addFillet(targetPartId, radius)`. Single undo entry.
 *
 * Outside an XR session this component renders nothing and does no work.
 *
 * The pose math runs in Three.js world space, which is also the WebXR
 * reference space, so we can compare hand positions directly against the
 * raycaster's intersect results without manual coordinate transforms — the
 * `XRSceneTransform` group's scale is handled implicitly by the raycaster.
 */

const PINCH_CLOSE = 0.025; // 2.5 cm — index-tip to thumb-tip
const PINCH_OPEN = 0.04; // hysteresis: must open past this to count as released
const REACH_RADIUS = 0.4; // 40 cm sphere of "I'm grabbing this"
const MIN_RADIUS_M = 0.005; // 5 mm physical at desktop scale; below this we skip
const MAX_RADIUS_M = 0.5;
/** Convert meters of separation to millimeters of model-space radius. The
 * XR transform scales the scene by 1/1000, so 1 physical metre between
 * hands = 1000 mm of fillet radius. */
const M_TO_MODEL_MM = 1000;

interface PinchSample {
  pinching: boolean;
  midpoint: THREE.Vector3;
  /** Direction from wrist toward pinch midpoint — used as a "reach ray". */
  reach: THREE.Vector3;
}

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

interface FilletCapture {
  partId: string;
  initialMidpoint: THREE.Vector3;
}

interface HandState {
  pinching: boolean;
  midpoint: THREE.Vector3;
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
  const captureRef = useRef<FilletCapture | null>(null);
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

    let leftSample: HandState | null = null;
    let rightSample: HandState | null = null;

    for (const src of session.inputSources) {
      if (!src.hand) continue;
      const handRef = src.handedness === "left" ? leftRef : rightRef;
      const sample = sampleRef.current;
      if (!samplePinch(xrFrame, src.hand, refSpace, handRef.current.pinching, sample)) {
        continue;
      }
      handRef.current.pinching = sample.pinching;
      handRef.current.midpoint.copy(sample.midpoint);
      const snapshot: HandState = {
        pinching: sample.pinching,
        midpoint: sample.midpoint.clone(),
      };
      if (src.handedness === "left") {
        leftSample = snapshot;
      } else if (src.handedness === "right") {
        rightSample = snapshot;
      }

      // First-pinch capture: if no target yet and this hand just pinched,
      // try to pick a part under the reach ray.
      if (sample.pinching && !captureRef.current) {
        const partId = pickPartId(scene, sample.midpoint, sample.reach);
        if (partId) {
          captureRef.current = {
            partId,
            initialMidpoint: sample.midpoint.clone(),
          };
        }
      }
    }

    // Commit on release. We commit when at least one of the hands transitions
    // out of pinch while a capture is active and the *other* hand is/was
    // pinching too (i.e. we had a bimanual moment). Single hand alone just
    // selects the part — no fillet.
    const capture = captureRef.current;
    if (!capture) return;

    const left = leftSample ?? leftRef.current;
    const right = rightSample ?? rightRef.current;
    const bothPinching = left.pinching && right.pinching;
    const eitherReleased = !left.pinching || !right.pinching;

    if (bothPinching || !eitherReleased) {
      // Still in capture; nothing to commit yet. (Live preview lands in a
      // follow-up — for now we just hold state.)
      return;
    }

    // Released. If we ever hit bimanual (both midpoints are valid and the
    // distance is non-degenerate), commit the fillet.
    const distance = left.midpoint.distanceTo(right.midpoint);
    captureRef.current = null;
    if (distance < MIN_RADIUS_M || distance > MAX_RADIUS_M) return;
    const radius = distance * M_TO_MODEL_MM;
    useDocumentStore.getState().addFillet(capture.partId, radius);
  });

  return null;
}
