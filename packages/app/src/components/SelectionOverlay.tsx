import { useMemo } from "react";
import * as THREE from "three";
import { Line } from "@react-three/drei";
import { useUiStore, useDocumentStore, useEngineStore } from "@vcad/core";
import { useTheme } from "@/hooks/useTheme";

const ACCENT_DARK = "#6b8fa3";
const ACCENT_LIGHT = "#4a7080";

function BoundingBoxLines({ box, color }: { box: THREE.Box3; color: string }) {
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
          opacity={0.5}
        />
      ))}
    </>
  );
}

export function SelectionOverlay() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const isOrbiting = useUiStore((s) => s.isOrbiting);
  const parts = useDocumentStore((s) => s.parts);
  const scene = useEngineStore((s) => s.scene);
  const { isDark } = useTheme();

  const accentColor = isDark ? ACCENT_DARK : ACCENT_LIGHT;

  // Compute combined bounding box for all selected parts
  const box = useMemo(() => {
    if (selectedPartIds.size === 0 || !scene) {
      return null;
    }

    const combinedBox = new THREE.Box3();
    let hasValidBox = false;

    selectedPartIds.forEach((partId) => {
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

    if (!hasValidBox) {
      return null;
    }

    return combinedBox;
  }, [selectedPartIds, parts, scene]);

  // Skip rendering during orbit for performance, or when no selection
  if (isOrbiting || !box || isDraggingGizmo) return null;

  return (
    <>
      {/* Dashed wireframe bounding box */}
      <BoundingBoxLines box={box} color={accentColor} />

    </>
  );
}
