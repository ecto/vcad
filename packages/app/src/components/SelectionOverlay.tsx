import { useMemo, type ReactNode } from "react";
import * as THREE from "three";
import { Line } from "@react-three/drei";
import {
  useUiStore,
  useDocumentStore,
  useEngineStore,
  useParticipantStore,
  LOCAL_PARTICIPANT_ID,
} from "@vcad/core";
import { useTheme } from "@/hooks/useTheme";

const ACCENT_DARK = "#6b8fa3";
const ACCENT_LIGHT = "#4a7080";

function BoundingBoxLines({
  box,
  color,
  opacity,
}: {
  box: THREE.Box3;
  color: string;
  opacity: number;
}) {
  const min = box.min;
  const max = box.max;

  // 12 edges of a box as line segments
  const edges: [THREE.Vector3, THREE.Vector3][] = [
    // Bottom face
    [
      new THREE.Vector3(min.x, min.y, min.z),
      new THREE.Vector3(max.x, min.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, min.z),
      new THREE.Vector3(max.x, min.y, max.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, max.z),
      new THREE.Vector3(min.x, min.y, max.z),
    ],
    [
      new THREE.Vector3(min.x, min.y, max.z),
      new THREE.Vector3(min.x, min.y, min.z),
    ],
    // Top face
    [
      new THREE.Vector3(min.x, max.y, min.z),
      new THREE.Vector3(max.x, max.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, max.y, min.z),
      new THREE.Vector3(max.x, max.y, max.z),
    ],
    [
      new THREE.Vector3(max.x, max.y, max.z),
      new THREE.Vector3(min.x, max.y, max.z),
    ],
    [
      new THREE.Vector3(min.x, max.y, max.z),
      new THREE.Vector3(min.x, max.y, min.z),
    ],
    // Vertical edges
    [
      new THREE.Vector3(min.x, min.y, min.z),
      new THREE.Vector3(min.x, max.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, min.z),
      new THREE.Vector3(max.x, max.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, max.z),
      new THREE.Vector3(max.x, max.y, max.z),
    ],
    [
      new THREE.Vector3(min.x, min.y, max.z),
      new THREE.Vector3(min.x, max.y, max.z),
    ],
  ];

  return (
    <>
      {edges.map((edge, i) => (
        <Line
          key={i}
          points={edge}
          color={color}
          lineWidth={1}
          dashed
          dashSize={1}
          gapSize={0.8}
          transparent
          opacity={opacity}
        />
      ))}
    </>
  );
}

/** Compute the combined bbox (kernel Z-up) of a set of selected part ids. */
function useSelectionBox(partIdSet: Set<string>): THREE.Box3 | null {
  const parts = useDocumentStore((s) => s.parts);
  const scene = useEngineStore((s) => s.scene);

  return useMemo(() => {
    if (partIdSet.size === 0 || !scene) return null;

    const combinedBox = new THREE.Box3();
    let hasValidBox = false;

    partIdSet.forEach((partId) => {
      const partIndex = parts.findIndex((p) => p.id === partId);
      if (partIndex === -1) return;

      const evalPart = scene.parts[partIndex];
      if (!evalPart) return;

      const mesh = evalPart.mesh;
      if (!mesh.positions.length) return;

      const partBox = new THREE.Box3();
      const pos = new THREE.Vector3();
      for (let i = 0; i < mesh.positions.length; i += 3) {
        pos.set(
          mesh.positions[i]!,
          mesh.positions[i + 1]!,
          mesh.positions[i + 2]!,
        );
        partBox.expandByPoint(pos);
      }

      combinedBox.union(partBox);
      hasValidBox = true;
    });

    return hasValidBox ? combinedBox : null;
  }, [partIdSet, parts, scene]);
}

/** Local user's selection box — dashed accent outline. */
function LocalSelection() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const box = useSelectionBox(selectedPartIds);
  const { isDark } = useTheme();
  if (!box) return null;
  const accentColor = isDark ? ACCENT_DARK : ACCENT_LIGHT;
  return <BoundingBoxLines box={box} color={accentColor} opacity={0.5} />;
}

/**
 * Every non-local participant's selection, drawn in their own color so
 * the user can see at a glance what the AI (or a peer) is looking at.
 */
function ParticipantAttention() {
  const participants = useParticipantStore((s) => s.participants);
  const followMode = useUiStore((s) => s.followMode);
  // Free mode hides all non-local presence cues.
  if (followMode === "free") return null;
  const nodes: ReactNode[] = [];
  participants.forEach((p) => {
    if (p.id === LOCAL_PARTICIPANT_ID) return;
    if (p.selectedPartIds.size === 0) return;
    nodes.push(<ParticipantSelection key={p.id} ids={p.selectedPartIds} color={p.color} />);
  });
  return <>{nodes}</>;
}

function ParticipantSelection({ ids, color }: { ids: Set<string>; color: string }) {
  const box = useSelectionBox(ids);
  if (!box) return null;
  // AI/peer attention is slightly brighter than local selection so it
  // reads as an event drawing your eye.
  return <BoundingBoxLines box={box} color={color} opacity={0.7} />;
}

export function SelectionOverlay() {
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const isOrbiting = useUiStore((s) => s.isOrbiting);

  // Skip rendering during orbit for performance.
  if (isOrbiting || isDraggingGizmo) return null;

  return (
    <>
      <LocalSelection />
      <ParticipantAttention />
    </>
  );
}
