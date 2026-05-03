import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import {
  joinXRCollabChannel,
  type XRCollabChannel,
  useAuthStore,
} from "@vcad/auth";
import { useXRPresenting, useXRSupportStore } from "@/stores/xr-store";
import { useCollabSessionStore } from "@/stores/collab-session-store";
import { useXRPresenceStore } from "@/stores/xr-presence-store";

/**
 * XR presence — broadcasts the local headset + hand poses on a per-document
 * realtime channel and renders incoming peer poses as floating avatars.
 *
 * Mounted *inside* `XRSceneTransform` so its rendered avatars share the same
 * scene-local frame as the kernel geometry. Poses are exchanged in this
 * scene-local frame too, so two users on different desks still see each
 * other's heads and hands at the same point on the same model.
 *
 * The local broadcast is throttled to ~10 Hz (every 100 ms). This is
 * smooth enough for floating avatars without saturating the realtime
 * channel — interpolation on the receive side absorbs the gaps.
 *
 * Outside XR, or while the document isn't cloud-synced, this component
 * renders nothing and never opens a channel.
 */

/** Broadcast period in ms. ~10 Hz. */
const SEND_PERIOD = 100;
/** Sweep stale peers at this period. */
const PRUNE_PERIOD = 2_000;

const _camPos = new THREE.Vector3();
const _camQuat = new THREE.Quaternion();
const _scenePos = new THREE.Vector3();

/** Convert a world-space position into the scene-local frame defined by
 * the current `XRSceneTransform` (`scale`, `position`). */
function worldToScene(
  world: THREE.Vector3,
  out: THREE.Vector3,
  view: { scale: number; position: readonly [number, number, number] },
) {
  out
    .copy(world)
    .sub(new THREE.Vector3(view.position[0], view.position[1], view.position[2]))
    .divideScalar(view.scale);
}

export function XRPresence() {
  const presenting = useXRPresenting();
  const { camera, gl } = useThree();
  const cloudId = useCollabSessionStore((s) => s.cloudId);
  const user = useAuthStore((s) => s.user);
  const userName = user?.user_metadata?.name as string | undefined;

  const channelRef = useRef<XRCollabChannel | null>(null);
  const lastSentRef = useRef(0);

  // Open / close the channel when XR + cloud-sync are both available.
  useEffect(() => {
    if (!presenting || !cloudId) return;
    const ingest = useXRPresenceStore.getState().ingest;
    const drop = useXRPresenceStore.getState().drop;
    const channel = joinXRCollabChannel(cloudId, ingest, drop);
    channelRef.current = channel;
    return () => {
      channel?.leave();
      channelRef.current = null;
      useXRPresenceStore.getState().clear();
    };
  }, [presenting, cloudId]);

  // Periodically prune peers we haven't heard from recently — covers the
  // case where a peer crashed without sending a `leave` event.
  useEffect(() => {
    if (!presenting) return;
    const interval = window.setInterval(
      () => useXRPresenceStore.getState().pruneStale(),
      PRUNE_PERIOD,
    );
    return () => window.clearInterval(interval);
  }, [presenting]);

  // Throttled broadcast of local pose in scene-local coords.
  useFrame((_, __, frame) => {
    if (!presenting || !channelRef.current) return;
    const now = performance.now();
    if (now - lastSentRef.current < SEND_PERIOD) return;
    lastSentRef.current = now;

    const view = useXRSupportStore.getState().view;

    // Headset world pose. In WebXR the camera tracks the head.
    camera.getWorldPosition(_camPos);
    camera.getWorldQuaternion(_camQuat);
    worldToScene(_camPos, _scenePos, view);

    // Hand wrist positions, scene-local. Optional — not all sessions have
    // hand tracking.
    let leftHand: [number, number, number] | undefined;
    let rightHand: [number, number, number] | undefined;
    const xrFrame = frame as XRFrame | undefined;
    const session = gl.xr.getSession();
    const refSpace = gl.xr.getReferenceSpace();
    if (xrFrame && session && refSpace) {
      for (const src of session.inputSources) {
        if (!src.hand) continue;
        const wrist = src.hand.get("wrist");
        if (!wrist) continue;
        const pose = xrFrame.getJointPose?.(wrist, refSpace);
        if (!pose) continue;
        const w = new THREE.Vector3(
          pose.transform.position.x,
          pose.transform.position.y,
          pose.transform.position.z,
        );
        const local = new THREE.Vector3();
        worldToScene(w, local, view);
        const triple: [number, number, number] = [local.x, local.y, local.z];
        if (src.handedness === "left") leftHand = triple;
        else if (src.handedness === "right") rightHand = triple;
      }
    }

    channelRef.current.broadcast({
      name: userName,
      pose: {
        head: [_scenePos.x, _scenePos.y, _scenePos.z],
        headRot: [_camQuat.x, _camQuat.y, _camQuat.z, _camQuat.w],
        leftHand,
        rightHand,
      },
    });
  });

  return <RemoteAvatars />;
}

/** Renders one floating avatar per remote peer. */
function RemoteAvatars() {
  const peers = useXRPresenceStore((s) => s.peers);
  const entries = Array.from(peers.values());
  if (entries.length === 0) return null;
  return (
    <group>
      {entries.map((p) => (
        <RemoteAvatar key={p.userId} update={p} />
      ))}
    </group>
  );
}

interface RemoteAvatarProps {
  update: import("@vcad/auth").XRPresenceUpdate;
}

const _q = new THREE.Quaternion();
const _e = new THREE.Euler();

function RemoteAvatar({ update }: RemoteAvatarProps) {
  const color = update.color ?? "#c084fc";
  const head = update.pose.head;
  _q.set(
    update.pose.headRot[0],
    update.pose.headRot[1],
    update.pose.headRot[2],
    update.pose.headRot[3],
  );
  _e.setFromQuaternion(_q);

  return (
    <group>
      {/* Head — small rounded box facing the peer's gaze direction. The
          scene-local frame is in kernel mm, so 100-unit cube ≈ 10 cm at
          desktop scale. */}
      <mesh position={head} rotation={[_e.x, _e.y, _e.z]}>
        <boxGeometry args={[140, 100, 100]} />
        <meshStandardMaterial color={color} emissive={color} emissiveIntensity={0.3} />
      </mesh>
      {/* Hand wrists — small spheres so the peer's gestures read at a
          glance even if hand-tracking isn't available on their headset. */}
      {update.pose.leftHand && (
        <mesh position={update.pose.leftHand}>
          <sphereGeometry args={[40, 12, 12]} />
          <meshStandardMaterial color={color} emissive={color} emissiveIntensity={0.25} />
        </mesh>
      )}
      {update.pose.rightHand && (
        <mesh position={update.pose.rightHand}>
          <sphereGeometry args={[40, 12, 12]} />
          <meshStandardMaterial color={color} emissive={color} emissiveIntensity={0.25} />
        </mesh>
      )}
    </group>
  );
}
