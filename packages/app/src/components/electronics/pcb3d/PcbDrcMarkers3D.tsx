/**
 * Animated ring markers at DRC violation positions.
 */

import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { layerZ } from "./pcb-geometry";
import type { DrcViolationResult } from "@vcad/engine";

interface Props {
  violations: DrcViolationResult[];
  boardThickness: number;
  explosion: number;
}

function DrcRing({ violation, z }: { violation: DrcViolationResult; z: number }) {
  const meshRef = useRef<THREE.Mesh>(null);

  useFrame(({ clock }) => {
    if (!meshRef.current) return;
    const t = clock.getElapsedTime();
    const scale = 0.6 + 0.4 * Math.sin(t * 3);
    meshRef.current.scale.set(scale, scale, 1);
  });

  const color = violation.severity === "Error" ? "#ef4444" : "#f59e0b";

  return (
    <mesh
      ref={meshRef}
      position={[violation.position.x, violation.position.y, z]}
    >
      <ringGeometry args={[0.6, 0.8, 24]} />
      <meshBasicMaterial color={color} transparent opacity={0.8} side={THREE.DoubleSide} />
    </mesh>
  );
}

export function PcbDrcMarkers3D({ violations, boardThickness, explosion }: Props) {
  if (violations.length === 0) return null;

  const z = layerZ("FCu", boardThickness, explosion) + 0.15;

  return (
    <group>
      {violations.map((v, i) => (
        <DrcRing key={i} violation={v} z={z} />
      ))}
    </group>
  );
}
