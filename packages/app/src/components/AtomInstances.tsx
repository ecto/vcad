/**
 * Instanced renderer for atomic structures (the `molecule` document domain).
 *
 * One `THREE.InstancedMesh` of an icosphere for atoms (CPK-colored, van-der-
 * Waals / covalent radii) and one of a cylinder for bonds — the same
 * `setMatrixAt` / `setColorAt` instancing pattern as the PCB via/pad meshes,
 * which scales to 10^5–10^6 atoms comfortably. Positions are in Å in the
 * kernel's Z-up frame; the viewport's global -90° X rotation handles Z-up→Y-up.
 *
 * This is Track A (interactive). Track B (the ray tracer's impostor-sphere
 * buffer) attaches via `RayTracer.uploadAtoms` for the high-quality path.
 */

import { useRef, useLayoutEffect, useMemo } from "react";
import * as THREE from "three";
import type { MoleculeSystem } from "@vcad/ir";

/** Rendering representation. */
export type AtomRepresentation = "ball_and_stick" | "space_filling" | "wireframe";

interface Props {
  molecule: MoleculeSystem;
  representation?: AtomRepresentation;
}

// CPK fallback colors (sRGB) for common elements; species.color overrides.
const CPK: Record<string, [number, number, number]> = {
  H: [1, 1, 1],
  C: [0.56, 0.56, 0.56],
  N: [0.19, 0.31, 0.97],
  O: [1, 0.05, 0.05],
  F: [0.56, 0.88, 0.31],
  Na: [0.67, 0.36, 0.95],
  Mg: [0.54, 1, 0],
  Al: [0.75, 0.65, 0.65],
  Si: [0.94, 0.78, 0.63],
  P: [1, 0.5, 0],
  S: [1, 1, 0.19],
  Cl: [0.12, 0.94, 0.12],
  K: [0.56, 0.25, 0.83],
  Ca: [0.24, 1, 0],
  Fe: [0.88, 0.4, 0.2],
  Cu: [0.78, 0.5, 0.2],
  Zn: [0.49, 0.5, 0.69],
  Au: [1, 0.82, 0.14],
};
// Covalent radii (Å) for a handful of common elements; else 0.75.
const COVALENT: Record<string, number> = {
  H: 0.31,
  C: 0.76,
  N: 0.71,
  O: 0.66,
  F: 0.57,
  P: 1.07,
  S: 1.05,
  Cl: 1.02,
  Fe: 1.32,
};

const TMP_MAT = new THREE.Matrix4();
const TMP_POS = new THREE.Vector3();
const TMP_SCALE = new THREE.Vector3();
const TMP_QUAT = new THREE.Quaternion();
const TMP_COLOR = new THREE.Color();
const Y_AXIS = new THREE.Vector3(0, 1, 0);
const TMP_DIR = new THREE.Vector3();
const TMP_A = new THREE.Vector3();
const TMP_B = new THREE.Vector3();

function elementColor(mol: MoleculeSystem, i: number): [number, number, number] {
  const sp = mol.species[mol.speciesIdx[i]!];
  if (sp?.color) return sp.color;
  return CPK[sp?.element ?? "C"] ?? [0.85, 0.4, 0.85];
}

function atomRadius(mol: MoleculeSystem, i: number, rep: AtomRepresentation): number {
  const sp = mol.species[mol.speciesIdx[i]!];
  const base = sp?.radius ?? COVALENT[sp?.element ?? "C"] ?? 0.75;
  if (rep === "space_filling") return base * 1.8;
  if (rep === "wireframe") return base * 0.18;
  return base * 0.4; // ball-and-stick
}

export function AtomInstances({ molecule, representation = "ball_and_stick" }: Props) {
  const atomsRef = useRef<THREE.InstancedMesh>(null);
  const bondsRef = useRef<THREE.InstancedMesh>(null);

  const atomCount = molecule.positions.length;
  const bonds = molecule.bonds ?? [];
  const showBonds = representation !== "space_filling";

  // Shared geometries/material, memoized by count-independent params.
  const atomGeometry = useMemo(() => new THREE.IcosahedronGeometry(1, 2), []);
  const bondGeometry = useMemo(() => new THREE.CylinderGeometry(1, 1, 1, 10), []);
  const material = useMemo(
    () => new THREE.MeshStandardMaterial({ roughness: 0.35, metalness: 0.0, vertexColors: false }),
    [],
  );
  const bondMaterial = useMemo(
    () => new THREE.MeshStandardMaterial({ color: "#9aa0a6", roughness: 0.5, metalness: 0.0 }),
    [],
  );

  useLayoutEffect(() => {
    const inst = atomsRef.current;
    if (!inst) return;
    for (let i = 0; i < atomCount; i++) {
      const p = molecule.positions[i]!;
      const r = atomRadius(molecule, i, representation);
      TMP_POS.set(p[0], p[1], p[2]);
      TMP_SCALE.set(r, r, r);
      TMP_QUAT.identity();
      TMP_MAT.compose(TMP_POS, TMP_QUAT, TMP_SCALE);
      inst.setMatrixAt(i, TMP_MAT);
      const [cr, cg, cb] = elementColor(molecule, i);
      TMP_COLOR.setRGB(cr, cg, cb);
      inst.setColorAt(i, TMP_COLOR);
    }
    inst.count = atomCount;
    inst.instanceMatrix.needsUpdate = true;
    if (inst.instanceColor) inst.instanceColor.needsUpdate = true;
  }, [molecule, atomCount, representation]);

  useLayoutEffect(() => {
    const inst = bondsRef.current;
    if (!inst || !showBonds) return;
    const radius = representation === "wireframe" ? 0.06 : 0.12;
    for (let b = 0; b < bonds.length; b++) {
      const bond = bonds[b]!;
      const pa = molecule.positions[bond.a]!;
      const pb = molecule.positions[bond.b]!;
      TMP_A.set(pa[0], pa[1], pa[2]);
      TMP_B.set(pb[0], pb[1], pb[2]);
      TMP_DIR.subVectors(TMP_B, TMP_A);
      const len = TMP_DIR.length();
      if (len < 1e-6) {
        TMP_MAT.makeScale(0, 0, 0);
        inst.setMatrixAt(b, TMP_MAT);
        continue;
      }
      TMP_DIR.normalize();
      TMP_QUAT.setFromUnitVectors(Y_AXIS, TMP_DIR);
      TMP_POS.addVectors(TMP_A, TMP_B).multiplyScalar(0.5);
      TMP_SCALE.set(radius, len, radius);
      TMP_MAT.compose(TMP_POS, TMP_QUAT, TMP_SCALE);
      inst.setMatrixAt(b, TMP_MAT);
    }
    inst.count = bonds.length;
    inst.instanceMatrix.needsUpdate = true;
  }, [molecule, bonds, showBonds, representation]);

  if (atomCount === 0) return null;

  return (
    <group>
      <instancedMesh
        ref={atomsRef}
        args={[atomGeometry, material, atomCount]}
        frustumCulled={false}
      />
      {showBonds && bonds.length > 0 && (
        <instancedMesh
          ref={bondsRef}
          args={[bondGeometry, bondMaterial, bonds.length]}
          frustumCulled={false}
        />
      )}
    </group>
  );
}
