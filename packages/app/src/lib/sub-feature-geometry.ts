/**
 * Geometry helpers for sub-feature highlights.
 *
 * Pulls all-triangles-coplanar-with-this-one out of `SceneMesh` so the new
 * `SelectionOverlay` can render face highlights using the same math the
 * sketch-mode face hover uses.
 *
 * Edge / vertex helpers translate `edgeId`/`vertexId` (from
 * `pickSubFeature`) back into the actual mesh positions to draw.
 */

import * as THREE from "three";
import type { TriangleMesh } from "@vcad/engine";
import { decodeEdgeId } from "./sub-feature-picking";

/** Tolerance for treating two normals as parallel and two coplanar
 *  triangles as on the same plane. */
const NORMAL_TOLERANCE = 0.01;
const PLANE_TOLERANCE = 0.01;

/** Find every triangle index that lies on the same infinite plane as the
 *  reference triangle. Used to expand a single triangle hit into the full
 *  face's highlight. */
export function findCoplanarTriangles(
  mesh: TriangleMesh,
  refTriIndex: number,
): number[] {
  const indices = mesh.indices;
  const positions = mesh.positions;

  const ri0 = indices[refTriIndex * 3]!;
  const ri1 = indices[refTriIndex * 3 + 1]!;
  const ri2 = indices[refTriIndex * 3 + 2]!;
  const r0 = new THREE.Vector3(
    positions[ri0 * 3]!,
    positions[ri0 * 3 + 1]!,
    positions[ri0 * 3 + 2]!,
  );
  const r1 = new THREE.Vector3(
    positions[ri1 * 3]!,
    positions[ri1 * 3 + 1]!,
    positions[ri1 * 3 + 2]!,
  );
  const r2 = new THREE.Vector3(
    positions[ri2 * 3]!,
    positions[ri2 * 3 + 1]!,
    positions[ri2 * 3 + 2]!,
  );
  const refNormal = r1.clone().sub(r0).cross(r2.clone().sub(r0)).normalize();
  const refOffset = refNormal.dot(r0);

  const triCount = indices.length / 3;
  const out: number[] = [];
  const v0 = new THREE.Vector3();
  const v1 = new THREE.Vector3();
  const v2 = new THREE.Vector3();
  const _e1 = new THREE.Vector3();
  const _e2 = new THREE.Vector3();
  const _n = new THREE.Vector3();
  for (let t = 0; t < triCount; t++) {
    const i0 = indices[t * 3]!;
    const i1 = indices[t * 3 + 1]!;
    const i2 = indices[t * 3 + 2]!;
    v0.set(positions[i0 * 3]!, positions[i0 * 3 + 1]!, positions[i0 * 3 + 2]!);
    v1.set(positions[i1 * 3]!, positions[i1 * 3 + 1]!, positions[i1 * 3 + 2]!);
    v2.set(positions[i2 * 3]!, positions[i2 * 3 + 1]!, positions[i2 * 3 + 2]!);
    _e1.subVectors(v1, v0);
    _e2.subVectors(v2, v0);
    _n.copy(_e1).cross(_e2).normalize();
    if (_n.dot(refNormal) < 1 - NORMAL_TOLERANCE) continue;
    if (Math.abs(refNormal.dot(v0) - refOffset) > PLANE_TOLERANCE) continue;
    out.push(t);
  }
  return out;
}

/** Build a geometry covering only the given triangle indices. Caller owns
 *  disposal of the returned geometry. */
export function buildFaceHighlightGeometry(
  mesh: TriangleMesh,
  triangleIndices: number[],
): THREE.BufferGeometry {
  const geo = new THREE.BufferGeometry();
  const buf: number[] = [];
  for (const t of triangleIndices) {
    const i0 = mesh.indices[t * 3]!;
    const i1 = mesh.indices[t * 3 + 1]!;
    const i2 = mesh.indices[t * 3 + 2]!;
    buf.push(
      mesh.positions[i0 * 3]!,
      mesh.positions[i0 * 3 + 1]!,
      mesh.positions[i0 * 3 + 2]!,
      mesh.positions[i1 * 3]!,
      mesh.positions[i1 * 3 + 1]!,
      mesh.positions[i1 * 3 + 2]!,
      mesh.positions[i2 * 3]!,
      mesh.positions[i2 * 3 + 1]!,
      mesh.positions[i2 * 3 + 2]!,
    );
  }
  geo.setAttribute("position", new THREE.Float32BufferAttribute(buf, 3));
  geo.computeVertexNormals();
  return geo;
}

/** Get a vertex's kernel-space position from a mesh by index. */
export function getVertex(mesh: TriangleMesh, vertexIndex: number): THREE.Vector3 {
  const i = vertexIndex * 3;
  return new THREE.Vector3(
    mesh.positions[i] ?? 0,
    mesh.positions[i + 1] ?? 0,
    mesh.positions[i + 2] ?? 0,
  );
}

/** Decode an edgeId into its two endpoint positions. */
export function getEdgeEndpoints(
  mesh: TriangleMesh,
  edgeId: number,
): { a: THREE.Vector3; b: THREE.Vector3 } {
  const vertexCount = mesh.positions.length / 3;
  const { a, b } = decodeEdgeId(edgeId, vertexCount);
  return { a: getVertex(mesh, a), b: getVertex(mesh, b) };
}
