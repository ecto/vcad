import { useState, useRef } from "react";
import { useFrame, useThree, ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import { useSketchStore } from "@vcad/core";
import type { AxisAlignedPlane } from "@vcad/core";

const GIZMO_SIZE = 8;
const GIZMO_OPACITY = 0.3;
const GIZMO_HOVER_OPACITY = 0.6;

// Colors matching the axis colors from GizmoViewport
const PLANE_COLORS: Record<AxisAlignedPlane, string> = {
  XY: "#61afef", // blue - Z axis color (plane perpendicular to Z)
  XZ: "#98c379", // green - Y axis color (plane perpendicular to Y)
  YZ: "#e06c75", // red - X axis color (plane perpendicular to X)
};

interface PlaneProps {
  plane: AxisAlignedPlane;
  rotation: [number, number, number];
  position: [number, number, number];
}

function GizmoPlane({ plane, rotation, position }: PlaneProps) {
  const [hovered, setHovered] = useState(false);
  const meshRef = useRef<THREE.Mesh>(null);
  const enterSketchMode = useSketchStore((s) => s.enterSketchMode);
  const sketchActive = useSketchStore((s) => s.active);
  const { invalidate } = useThree();

  const handleClick = (e: ThreeEvent<MouseEvent>) => {
    e.stopPropagation();
    if (sketchActive) return;

    // Detect which side of the plane was clicked
    // Face normal in local space transformed to world space
    let flipped = false;
    if (e.face?.normal && meshRef.current) {
      const worldNormal = e.face.normal
        .clone()
        .transformDirection(meshRef.current.matrixWorld);
      // Ray direction points into the scene; if dot product > 0, we clicked the back face
      flipped = worldNormal.dot(e.ray.direction) > 0;
    }

    enterSketchMode(plane, undefined, flipped);
  };

  // Animate opacity on hover — request frames until converged
  useFrame(() => {
    if (!meshRef.current) return;
    const material = meshRef.current.material as THREE.MeshBasicMaterial;
    const targetOpacity = hovered ? GIZMO_HOVER_OPACITY : GIZMO_OPACITY;
    const delta = targetOpacity - material.opacity;
    if (Math.abs(delta) < 0.01) {
      material.opacity = targetOpacity;
      return;
    }
    material.opacity += delta * 0.2;
    invalidate();
  });

  return (
    <mesh
      ref={meshRef}
      position={position}
      rotation={rotation}
      onPointerEnter={(e) => {
        e.stopPropagation();
        if (!sketchActive) {
          setHovered(true);
          document.body.style.cursor = "pointer";
        }
      }}
      onPointerLeave={() => {
        setHovered(false);
        document.body.style.cursor = "auto";
      }}
      onClick={handleClick}
    >
      <planeGeometry args={[GIZMO_SIZE, GIZMO_SIZE]} />
      <meshBasicMaterial
        color={PLANE_COLORS[plane]}
        transparent
        opacity={GIZMO_OPACITY}
        side={THREE.DoubleSide}
        depthWrite={false}
        depthTest={false}
      />
    </mesh>
  );
}

export function PlaneGizmo() {
  const sketchActive = useSketchStore((s) => s.active);

  // Hide gizmo when sketch is active
  if (sketchActive) return null;

  const halfSize = GIZMO_SIZE / 2;

  return (
    <group>
      {/* XY plane - lies flat at Z=0, offset in +X +Y quadrant */}
      <GizmoPlane
        plane="XY"
        rotation={[0, 0, 0]}
        position={[halfSize, halfSize, 0]}
      />

      {/* XZ plane - vertical, faces Y direction, offset in +X +Z quadrant */}
      <GizmoPlane
        plane="XZ"
        rotation={[-Math.PI / 2, 0, 0]}
        position={[halfSize, 0, halfSize]}
      />

      {/* YZ plane - vertical, faces X direction, offset in +Y +Z quadrant */}
      <GizmoPlane
        plane="YZ"
        rotation={[0, Math.PI / 2, 0]}
        position={[0, halfSize, halfSize]}
      />
    </group>
  );
}
