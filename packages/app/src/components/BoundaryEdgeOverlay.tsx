/**
 * Visualize the boundary edges of a triangle mesh as bright red lines.
 *
 * Boundary edges (edges adjacent to exactly one triangle) appear only on
 * meshes with holes. Every red segment rendered here is a tessellation
 * gap — use this overlay to hunt down missing fillet blend faces.
 *
 * Toggle with the UI switch in the Viewport status bar.
 */
import { useMemo } from "react";
import * as THREE from "three";

interface Props {
  positions: Float32Array;
  indices: Uint32Array;
}

/** Compute `(min,max)` index pairs appearing exactly once across all triangles. */
function computeBoundaryEdges(
  positions: Float32Array,
  indices: Uint32Array,
): Float32Array {
  const counts = new Map<string, number>();
  const keyOf = (a: number, b: number) =>
    a < b ? `${a},${b}` : `${b},${a}`;

  const triCount = indices.length / 3;
  for (let t = 0; t < triCount; t++) {
    const i0 = indices[3 * t]!;
    const i1 = indices[3 * t + 1]!;
    const i2 = indices[3 * t + 2]!;
    for (const [a, b] of [
      [i0, i1],
      [i1, i2],
      [i2, i0],
    ] as [number, number][]) {
      const k = keyOf(a, b);
      counts.set(k, (counts.get(k) ?? 0) + 1);
    }
  }

  const segs: number[] = [];
  for (const [k, n] of counts) {
    if (n !== 1) continue;
    const [aStr, bStr] = k.split(",");
    const a = parseInt(aStr!, 10);
    const b = parseInt(bStr!, 10);
    segs.push(
      positions[3 * a]!,
      positions[3 * a + 1]!,
      positions[3 * a + 2]!,
      positions[3 * b]!,
      positions[3 * b + 1]!,
      positions[3 * b + 2]!,
    );
  }
  return new Float32Array(segs);
}

export function BoundaryEdgeOverlay({ positions, indices }: Props) {
  const lineGeom = useMemo(() => {
    const edgePositions = computeBoundaryEdges(positions, indices);
    const geom = new THREE.BufferGeometry();
    geom.setAttribute(
      "position",
      new THREE.BufferAttribute(edgePositions, 3),
    );
    return geom;
  }, [positions, indices]);

  // Bright red, unlit, on top of everything — depth test off so they
  // aren't buried inside the surface.
  return (
    <lineSegments geometry={lineGeom} renderOrder={999}>
      <lineBasicMaterial
        color="#ff2040"
        linewidth={2}
        depthTest={false}
        transparent
      />
    </lineSegments>
  );
}
