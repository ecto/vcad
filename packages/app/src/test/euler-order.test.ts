import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { transformMesh, type TransformInfo } from "@vcad/engine";

/**
 * Regression: the app's Transform3D rendering must agree with the kernel's
 * euler convention — extrinsic X→Y→Z (matrix Rz·Ry·Rx), i.e. three.js Euler
 * order "ZYX". Authorities: crates/vcad-eval/src/kinematics.rs euler_to_matrix
 * and packages/engine/src/evaluate.ts transformMesh. The app historically used
 * three.js's default "XYZ", which diverges for any multi-axis rotation.
 */

const DEG2RAD = Math.PI / 180;

/** Asymmetric sample points so no axis or order coincidence can hide a bug. */
const SAMPLE_POINTS: [number, number, number][] = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1],
  [2, 3, 5],
  [-1, 4, -2],
  [7, -6, 1.5],
];

const TRANSFORM: TransformInfo = {
  translate: { x: 10, y: -5, z: 3 },
  rotate: { x: 30, y: 45, z: 60 },
  scale: { x: 1, y: 2, z: 0.5 },
};

/** Points through the engine's authoritative transformMesh. */
function enginePoints(t: TransformInfo): number[] {
  const mesh = {
    positions: new Float32Array(SAMPLE_POINTS.flat()),
    indices: new Uint32Array(),
  };
  return Array.from(transformMesh(mesh, t).positions);
}

/**
 * Points through the app's render path: SceneMesh sets object position /
 * quaternion-from-euler / scale, which three.js composes as T·R·S.
 */
function appPoints(t: TransformInfo, order: THREE.EulerOrder): number[] {
  const matrix = new THREE.Matrix4().compose(
    new THREE.Vector3(t.translate.x, t.translate.y, t.translate.z),
    new THREE.Quaternion().setFromEuler(
      new THREE.Euler(
        t.rotate.x * DEG2RAD,
        t.rotate.y * DEG2RAD,
        t.rotate.z * DEG2RAD,
        order,
      ),
    ),
    new THREE.Vector3(t.scale.x, t.scale.y, t.scale.z),
  );
  return SAMPLE_POINTS.flatMap((p) => {
    const v = new THREE.Vector3(...p).applyMatrix4(matrix);
    return [v.x, v.y, v.z];
  });
}

function maxDeviation(a: number[], b: number[]): number {
  let max = 0;
  for (let i = 0; i < a.length; i++) {
    max = Math.max(max, Math.abs(a[i]! - b[i]!));
  }
  return max;
}

describe("Transform3D euler order (app render path vs kernel)", () => {
  it('app "ZYX" euler matches engine transformMesh for rotation (30,45,60)', () => {
    const engine = enginePoints(TRANSFORM);
    const app = appPoints(TRANSFORM, "ZYX");
    // Float32Array round-trip in the engine path limits precision to ~1e-6.
    expect(maxDeviation(engine, app)).toBeLessThan(1e-4);
  });

  it('three.js default "XYZ" diverges — guards that this test is sensitive', () => {
    const engine = enginePoints(TRANSFORM);
    const app = appPoints(TRANSFORM, "XYZ");
    expect(maxDeviation(engine, app)).toBeGreaterThan(1);
  });

  it("single-axis rotations agree regardless of order", () => {
    for (const axis of ["x", "y", "z"] as const) {
      const t: TransformInfo = {
        translate: { x: 0, y: 0, z: 0 },
        rotate: { x: 0, y: 0, z: 0, [axis]: 90 },
        scale: { x: 1, y: 1, z: 1 },
      };
      expect(maxDeviation(enginePoints(t), appPoints(t, "ZYX"))).toBeLessThan(1e-4);
      expect(maxDeviation(enginePoints(t), appPoints(t, "XYZ"))).toBeLessThan(1e-4);
    }
  });
});
