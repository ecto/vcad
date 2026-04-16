import { useEffect, useMemo, useRef, type ReactNode } from "react";
import * as THREE from "three";
import { useFrame, useThree } from "@react-three/fiber";
import { Html } from "@react-three/drei";
import {
  useParticipantStore,
  LOCAL_PARTICIPANT_ID,
  useUiStore,
} from "@vcad/core";
import type { CameraGoal, Participant } from "@vcad/core";

/**
 * Renders a wireframe frustum + floating name chip for every non-local
 * participant that has a camera opinion. Mount *inside* the
 * `<group rotation={[-Math.PI / 2, 0, 0]}>` in ViewportContent so the
 * kernel Z-up coordinates we store align with the rest of the scene.
 *
 * The frustum animates toward its goal via a simple lerp on every frame
 * so the AI's camera moves feel like a fly rather than a teleport.
 */

// FOV used purely for the icon's visual shape — doesn't need to match the
// user's actual camera FOV.
const ICON_HALF_FOV = Math.PI / 7; // ~25° half-angle

/** Fraction of the eye→target distance to use for the frustum "icon length". */
const FAR_FRAC = 0.25;
/** Hard caps so the frustum never looks dominant or invisible. */
const FAR_MIN = 6;
const FAR_MAX = 40;

function computeFrustumPoints(
  eye: THREE.Vector3,
  target: THREE.Vector3,
  outApex: THREE.Vector3,
  outCorners: [THREE.Vector3, THREE.Vector3, THREE.Vector3, THREE.Vector3],
): void {
  outApex.copy(eye);
  const forward = _tmpForward.copy(target).sub(eye);
  const dist = forward.length();
  if (dist < 1e-6) {
    forward.set(0, -1, 0);
  } else {
    forward.divideScalar(dist);
  }

  const far = Math.max(FAR_MIN, Math.min(FAR_MAX, dist * FAR_FRAC));
  const halfW = Math.tan(ICON_HALF_FOV) * far;

  // World-up in kernel Z-up space. Swap in Y-axis when forward aligns with Z.
  const worldUp = _tmpWorldUp.set(0, 0, 1);
  if (Math.abs(forward.dot(worldUp)) > 0.98) worldUp.set(0, 1, 0);

  const right = _tmpRight.crossVectors(forward, worldUp).normalize();
  const up = _tmpUp.crossVectors(right, forward).normalize();

  const farCenter = _tmpFarCenter.copy(eye).addScaledVector(forward, far);
  const dx = _tmpDx.copy(right).multiplyScalar(halfW);
  const dy = _tmpDy.copy(up).multiplyScalar(halfW);

  outCorners[0].copy(farCenter).add(dx).add(dy);
  outCorners[1].copy(farCenter).sub(dx).add(dy);
  outCorners[2].copy(farCenter).sub(dx).sub(dy);
  outCorners[3].copy(farCenter).add(dx).sub(dy);
}

// Reusable temp vectors (per-frame, single-threaded — safe to share).
const _tmpForward = new THREE.Vector3();
const _tmpWorldUp = new THREE.Vector3();
const _tmpRight = new THREE.Vector3();
const _tmpUp = new THREE.Vector3();
const _tmpFarCenter = new THREE.Vector3();
const _tmpDx = new THREE.Vector3();
const _tmpDy = new THREE.Vector3();

// 8 line segments: 4 rim + 4 spokes = 16 endpoints.
const SEGMENT_COUNT = 8;
const VERTEX_COUNT = SEGMENT_COUNT * 2;

interface FrustumProps {
  participant: Participant;
  /** Goal camera in kernel Z-up; the mesh lerps toward this every frame. */
  goal: CameraGoal;
}

function ParticipantFrustum({ participant, goal }: FrustumProps) {
  const invalidate = useThree((s) => s.invalidate);

  // Live animated eye + target, kernel Z-up.
  const eyeRef = useRef(new THREE.Vector3(...goal.position));
  const targetRef = useRef(new THREE.Vector3(...goal.target));
  const apexRef = useRef(new THREE.Vector3());
  const cornersRef = useRef<
    [THREE.Vector3, THREE.Vector3, THREE.Vector3, THREE.Vector3]
  >([
    new THREE.Vector3(),
    new THREE.Vector3(),
    new THREE.Vector3(),
    new THREE.Vector3(),
  ]);

  // Re-request a frame whenever the goal changes so the lerp can kick in
  // even when the viewport is otherwise idle (demand-rendering mode).
  useEffect(() => {
    invalidate();
  }, [goal.position[0], goal.position[1], goal.position[2], goal.target[0], goal.target[1], goal.target[2], invalidate]);

  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry();
    const positions = new Float32Array(VERTEX_COUNT * 3);
    geom.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    return geom;
  }, []);

  const material = useMemo(
    () =>
      new THREE.LineBasicMaterial({
        color: participant.color,
        transparent: true,
        opacity: 0.85,
        depthTest: true,
      }),
    [participant.color],
  );

  useEffect(
    () => () => {
      geometry.dispose();
      material.dispose();
    },
    [geometry, material],
  );

  const chipRef = useRef<THREE.Group | null>(null);

  useFrame(() => {
    const goalEye = _goalEye.set(
      goal.position[0],
      goal.position[1],
      goal.position[2],
    );
    const goalTarget = _goalTarget.set(
      goal.target[0],
      goal.target[1],
      goal.target[2],
    );
    const lerp = 0.15;
    eyeRef.current.lerp(goalEye, lerp);
    targetRef.current.lerp(goalTarget, lerp);

    computeFrustumPoints(
      eyeRef.current,
      targetRef.current,
      apexRef.current,
      cornersRef.current,
    );

    const pos = geometry.getAttribute("position") as THREE.BufferAttribute;
    const apex = apexRef.current;
    const corners = cornersRef.current;

    // Rim segments 0-1, 1-2, 2-3, 3-0 → vertices [0..7]
    for (let i = 0; i < 4; i++) {
      const a = corners[i]!;
      const b = corners[(i + 1) % 4]!;
      pos.setXYZ(i * 2, a.x, a.y, a.z);
      pos.setXYZ(i * 2 + 1, b.x, b.y, b.z);
    }
    // Spoke segments apex→corner[i] → vertices [8..15]
    for (let i = 0; i < 4; i++) {
      pos.setXYZ(8 + i * 2, apex.x, apex.y, apex.z);
      const c = corners[i]!;
      pos.setXYZ(8 + i * 2 + 1, c.x, c.y, c.z);
    }
    pos.needsUpdate = true;
    geometry.computeBoundingSphere();

    if (chipRef.current) chipRef.current.position.copy(apex);

    // Keep the demand renderer awake while the lerp is still converging.
    const eyeDelta = eyeRef.current.distanceToSquared(goalEye);
    const targetDelta = targetRef.current.distanceToSquared(goalTarget);
    if (eyeDelta > 1e-4 || targetDelta > 1e-4) invalidate();
  });

  return (
    <group>
      <lineSegments geometry={geometry} material={material} />
      <group ref={chipRef}>
        <Html
          center
          distanceFactor={40}
          style={{
            pointerEvents: "none",
            transform: "translate(0, -18px)",
            whiteSpace: "nowrap",
            fontSize: "11px",
            fontWeight: 600,
            padding: "2px 6px",
            borderRadius: "4px",
            color: "white",
            background: participant.color,
            boxShadow: "0 1px 3px rgba(0,0,0,0.3)",
          }}
        >
          {participant.name}
        </Html>
      </group>
    </group>
  );
}

const _goalEye = new THREE.Vector3();
const _goalTarget = new THREE.Vector3();

export function ParticipantCameraOverlay() {
  const participants = useParticipantStore((s) => s.participants);
  const followMode = useUiStore((s) => s.followMode);
  const followingId = useUiStore((s) => s.followingParticipantId);

  // Free mode means "I don't want to see the AI in my viewport at all." Hide
  // every frustum rather than just the selection color.
  if (followMode === "free") return null;

  const nodes: ReactNode[] = [];
  participants.forEach((p) => {
    if (p.id === LOCAL_PARTICIPANT_ID) return;
    if (!p.camera) return;
    // While locked to a participant, the user *is* that camera — drawing
    // its own frustum around the user's eye looks silly, so skip.
    if (followMode === "lock" && followingId === p.id) return;
    nodes.push(<ParticipantFrustum key={p.id} participant={p} goal={p.camera} />);
  });

  return <>{nodes}</>;
}
