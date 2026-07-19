import { useEffect, useRef } from "react";
import * as THREE from "three";
import type { TriangleMesh } from "@vcad/engine";
import { useVersionTimelineStore } from "@/stores/version-timeline-store";

const REMOVED_COLOR = "#ff4d4d";
const ADDED_COLOR = "#2fbf71";

function GhostMesh({ mesh, color }: { mesh: TriangleMesh; color: string }) {
  const geoRef = useRef<THREE.BufferGeometry>(null);

  useEffect(() => {
    const geo = geoRef.current;
    if (!geo) return;
    geo.setAttribute(
      "position",
      new THREE.BufferAttribute(new Float32Array(mesh.positions), 3),
    );
    geo.setIndex(new THREE.BufferAttribute(new Uint32Array(mesh.indices), 1));
    geo.computeVertexNormals();
    geo.computeBoundingSphere();
    return () => {
      geo.dispose();
    };
  }, [mesh]);

  return (
    <mesh renderOrder={997} raycast={() => null}>
      <bufferGeometry ref={geoRef} />
      <meshStandardMaterial
        color={color}
        transparent
        opacity={0.35}
        side={THREE.DoubleSide}
        depthWrite={false}
      />
    </mesh>
  );
}

/**
 * Before/after ghost overlay for the version timeline: geometry removed
 * since the parent version renders red, added geometry renders green.
 * Mounted inside the viewport's Z-up rotation group.
 */
export function VersionGhostOverlay() {
  const ghost = useVersionTimelineStore((s) => s.ghost);
  if (!ghost) return null;
  return (
    <group>
      {ghost.removed.map((mesh, i) => (
        <GhostMesh key={`r${i}`} mesh={mesh} color={REMOVED_COLOR} />
      ))}
      {ghost.added.map((mesh, i) => (
        <GhostMesh key={`a${i}`} mesh={mesh} color={ADDED_COLOR} />
      ))}
    </group>
  );
}
