/**
 * Renders the 3D component bodies on the board — the height-aware extrusions
 * (chip caps, IC bodies, pin headers, …) that the kernel already generates via
 * `generate_component_meshes`. Until now those meshes were computed only for
 * MCAD/ECAD interference testing and never drawn; rendering them turns the flat
 * copper-and-silkscreen board into a populated PCB you can read at a glance.
 *
 * A component whose body intersects a surrounding mechanical part (the
 * enclosure) is drawn in a warning tint so the clash is obvious in 3D — the
 * "will this tall cap hit the lid?" check, answered live.
 *
 * Mounted inside PcbScene's rotation group; `ComponentMesh.positions` are
 * board-local (kernel Z-up), so they line up with the board, pads and traces.
 */

import { useEffect, useMemo } from "react";
import * as THREE from "three";
import { useElectronicsStore } from "@/stores/electronics-store";

const INTERFERE_COLOR = new THREE.Color("#ef4444");
const BLACK = new THREE.Color(0, 0, 0);

export function PcbComponentBodies3D() {
  const bodies = useElectronicsStore((s) => s.componentBodies);
  const show = useElectronicsStore((s) => s.showComponentBodies);
  const interfering = useElectronicsStore((s) => s.interferingFootprints);

  const interferingSet = useMemo(() => new Set(interfering), [interfering]);

  const built = useMemo(() => {
    return bodies.map((m, i) => {
      const geo = new THREE.BufferGeometry();
      geo.setAttribute("position", new THREE.Float32BufferAttribute(m.positions, 3));
      if (m.normals.length === m.positions.length) {
        geo.setAttribute("normal", new THREE.Float32BufferAttribute(m.normals, 3));
      }
      geo.setIndex(m.indices);
      if (m.normals.length !== m.positions.length) geo.computeVertexNormals();
      const emissive = m.emissive ?? [0, 0, 0];
      return {
        key: `body-${i}`,
        geo,
        ref: m.footprint_ref,
        color: new THREE.Color(m.color[0], m.color[1], m.color[2]),
        metalness: m.metalness,
        roughness: m.roughness ?? 0.45,
        emissive: new THREE.Color(emissive[0], emissive[1], emissive[2]),
        emissiveOn: emissive[0] > 0 || emissive[1] > 0 || emissive[2] > 0,
      };
    });
  }, [bodies]);

  // Dispose the previous batch of geometries when the meshes change / unmount.
  useEffect(() => () => built.forEach((b) => b.geo.dispose()), [built]);

  if (!show || built.length === 0) return null;

  return (
    <group>
      {built.map((b) => {
        const clash = interferingSet.has(b.ref);
        return (
          <mesh key={b.key} geometry={b.geo} castShadow receiveShadow>
            <meshStandardMaterial
              color={clash ? INTERFERE_COLOR : b.color}
              metalness={clash ? 0.1 : b.metalness}
              roughness={clash ? 0.45 : b.roughness}
              emissive={
                clash ? INTERFERE_COLOR : b.emissiveOn ? b.emissive : BLACK
              }
              emissiveIntensity={clash ? 0.45 : b.emissiveOn ? 2.5 : 0}
            />
          </mesh>
        );
      })}
    </group>
  );
}

export default PcbComponentBodies3D;
