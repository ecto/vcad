/**
 * World-class instanced renderer for atomic structures.
 *
 * Atoms are drawn as **GPU impostor spheres**: one billboarded quad per atom,
 * with the sphere ray-traced analytically in the fragment shader. This is the
 * technique QuteMol / Mol* use — pixel-perfect silhouettes at any zoom, correct
 * per-fragment depth (so spheres interpenetrate and occlude precisely), and it
 * scales to millions of atoms because there is no sphere tessellation. Shading
 * is a self-contained molecular model (key/fill/rim + hemispheric ambient +
 * Blinn specular + Fresnel), ACES-tonemapped and sRGB-encoded to match the rest
 * of the viewport. Bonds are two-tone instanced cylinders, each half colored by
 * its atom.
 *
 * Positions are in Å in the kernel's Z-up frame; the viewport's global -90° X
 * rotation handles Z-up→Y-up.
 */

import { useMemo, useRef, useLayoutEffect, useEffect } from "react";
import { useFrame, type ThreeEvent } from "@react-three/fiber";
import * as THREE from "three";
import type { MoleculeSystem } from "@vcad/ir";
import { useMoleculeStore } from "../stores/molecule-store";
import { viewportWasDrag } from "@/lib/viewport-drag";

/** Rendering representation. */
export type AtomRepresentation = "ball_and_stick" | "space_filling" | "wireframe";

interface Props {
  molecule: MoleculeSystem;
  representation?: AtomRepresentation;
}

// CPK fallback colors (sRGB 0..1); species.color overrides.
const CPK: Record<string, [number, number, number]> = {
  H: [1, 1, 1],
  C: [0.34, 0.34, 0.34],
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
const COVALENT: Record<string, number> = {
  H: 0.31, C: 0.76, N: 0.71, O: 0.66, F: 0.57, Na: 1.66, Mg: 1.41,
  Al: 1.21, Si: 1.11, P: 1.07, S: 1.05, Cl: 1.02, K: 2.03, Ca: 1.76,
  Fe: 1.32, Cu: 1.32, Zn: 1.22, Au: 1.36,
};

function speciesColor(mol: MoleculeSystem, i: number): [number, number, number] {
  const sp = mol.species[mol.speciesIdx[i]!];
  return sp?.color ?? CPK[sp?.element ?? "C"] ?? [0.85, 0.4, 0.85];
}
function speciesRadius(mol: MoleculeSystem, i: number, rep: AtomRepresentation): number {
  const sp = mol.species[mol.speciesIdx[i]!];
  const base = sp?.radius ?? COVALENT[sp?.element ?? "C"] ?? 0.75;
  if (rep === "space_filling") return base * 1.8;
  if (rep === "wireframe") return base * 0.2;
  return base * 0.42;
}

// Shared shading model (view-space N, V). Self-lit so it reads as a dedicated
// molecular renderer, independent of scene lights.
const SHADE_GLSL = /* glsl */ `
vec3 acesFilm(vec3 x){
  const float a=2.51, b=0.03, c=2.43, d=0.59, e=0.14;
  return clamp((x*(a*x+b))/(x*(c*x+d)+e), 0.0, 1.0);
}
vec3 shadeMolecule(vec3 srgb, vec3 N, vec3 V){
  vec3 base = pow(srgb, vec3(2.2));                 // to linear
  vec3 keyDir  = normalize(vec3(0.35, 0.65, 0.68)); // camera-relative lights
  vec3 fillDir = normalize(vec3(-0.6, -0.2, 0.35));
  float key  = max(dot(N, keyDir), 0.0);
  float fill = max(dot(N, fillDir), 0.0) * 0.30;
  // hemispheric ambient (sky above / ground below in view space)
  float hemi = N.y * 0.5 + 0.5;
  vec3 ambient = mix(vec3(0.10,0.10,0.13), vec3(0.42,0.48,0.58), hemi);
  // Blinn specular from the key light
  vec3 H = normalize(keyDir + V);
  float spec = pow(max(dot(N, H), 0.0), 56.0) * 0.55;
  // Fresnel rim
  float fres = pow(1.0 - max(dot(N, V), 0.0), 3.0);
  vec3 rim = vec3(0.45, 0.62, 0.95) * fres * 0.6;
  // subtle edge AO so spheres feel volumetric
  float edgeAO = mix(0.72, 1.0, max(dot(N, V), 0.0));
  vec3 col = base * (ambient + key + fill) * edgeAO + spec + rim;
  col = acesFilm(col);
  return pow(col, vec3(1.0/2.2));                   // to sRGB
}`;

// --- Impostor sphere material --------------------------------------------
const SPHERE_VERT = /* glsl */ `
in vec3 iCenter;
in float iRadius;
in vec3 iColor;
in float iIndex;
out vec3 vColor;
out float vRadius;
out float vIndex;
out vec3 vCenterView;
out vec3 vViewPos;
void main(){
  vColor = iColor;
  vRadius = iRadius;
  vIndex = iIndex;
  vec4 cv = modelViewMatrix * vec4(iCenter, 1.0);
  vCenterView = cv.xyz;
  // camera-facing quad, oversized for perspective safety
  vec3 pos = cv.xyz + vec3(position.xy * iRadius * 1.5, 0.0);
  vViewPos = pos;
  gl_Position = projectionMatrix * vec4(pos, 1.0);
}`;

const SPHERE_FRAG = /* glsl */ `
precision highp float;
uniform float logDepthBufFC;
uniform float uSelected;       // selected atom index, or -1
uniform mat4 projectionMatrix; // three binds this built-in when declared in-stage
in vec3 vColor;
in float vRadius;
in float vIndex;
in vec3 vCenterView;
in vec3 vViewPos;
out vec4 fragColor;
${SHADE_GLSL}
void main(){
  vec3 rd = normalize(vViewPos);          // ray from camera (origin) to fragment
  vec3 oc = -vCenterView;
  float b = dot(oc, rd);
  float c = dot(oc, oc) - vRadius * vRadius;
  float h = b * b - c;
  if (h < 0.0) discard;                   // ray misses sphere
  float t = -b - sqrt(h);
  if (t < 0.0) discard;
  vec3 P = rd * t;                        // hit point (view space)
  vec3 N = normalize(P - vCenterView);
  vec3 V = normalize(-P);
  // correct depth for logarithmicDepthBuffer
  vec4 clip = projectionMatrix * vec4(P, 1.0);
  gl_FragDepth = log2(1.0 + clip.w) * logDepthBufFC * 0.5;
  vec3 col = shadeMolecule(vColor, N, V);
  // Selection highlight: warm emissive lift + a bright silhouette ring.
  if (abs(vIndex - uSelected) < 0.5) {
    float ring = smoothstep(0.55, 0.98, 1.0 - max(dot(N, V), 0.0));
    col = mix(col, vec3(1.0, 0.85, 0.35), 0.28) + vec3(1.0, 0.8, 0.3) * ring * 0.9;
  }
  fragColor = vec4(col, 1.0);
}`;

// --- Custom-lit bond material (real cylinder geometry) -------------------
// `instanceMatrix` / `instanceColor` are injected by three for InstancedMesh.
const BOND_VERT = /* glsl */ `
out vec3 vColor;
out vec3 vNormalV;
out vec3 vViewPos;
void main(){
  #ifdef USE_INSTANCING_COLOR
    vColor = instanceColor;
  #else
    vColor = vec3(0.6);
  #endif
  vec4 mv = modelViewMatrix * instanceMatrix * vec4(position, 1.0);
  vViewPos = mv.xyz;
  vNormalV = normalize((modelViewMatrix * instanceMatrix * vec4(normal, 0.0)).xyz);
  gl_Position = projectionMatrix * mv;
}`;

const BOND_FRAG = /* glsl */ `
precision highp float;
uniform float logDepthBufFC;
uniform mat4 projectionMatrix; // three binds this built-in when declared in-stage
in vec3 vColor;
in vec3 vNormalV;
in vec3 vViewPos;
out vec4 fragColor;
${SHADE_GLSL}
void main(){
  vec3 N = normalize(vNormalV);
  if (!gl_FrontFacing) N = -N;
  vec3 V = normalize(-vViewPos);
  vec4 clip = projectionMatrix * vec4(vViewPos, 1.0);
  gl_FragDepth = log2(1.0 + clip.w) * logDepthBufFC * 0.5;
  fragColor = vec4(shadeMolecule(vColor, N, V), 1.0);
}`;

export function AtomInstances({ molecule, representation = "ball_and_stick" }: Props) {
  const bondsRef = useRef<THREE.InstancedMesh>(null);
  const pickRef = useRef<THREE.InstancedMesh>(null);
  const selectAtom = useMoleculeStore((s) => s.selectAtom);

  const atomCount = molecule.positions.length;
  const bonds = molecule.bonds ?? [];
  const showBonds = representation !== "space_filling";

  // Molecules are authored in Ångström (~1–20 units); the viewport is framed
  // for millimeter CAD parts, so scale each structure to a comfortable display
  // size. Impostor radii live in view space, so the scale is baked into the
  // uploaded centers/radii rather than applied as a group transform.
  const displayScale = useMemo(() => {
    let maxR = 1e-3;
    for (const p of molecule.positions) maxR = Math.max(maxR, Math.hypot(p[0], p[1], p[2]));
    return 34 / maxR;
  }, [molecule]);

  // Impostor sphere geometry (instanced quads).
  const sphereGeom = useMemo(() => {
    const quad = new THREE.PlaneGeometry(2, 2);
    const geo = new THREE.InstancedBufferGeometry();
    geo.index = quad.index;
    geo.attributes.position = quad.attributes.position!;
    geo.attributes.uv = quad.attributes.uv!;
    const centers = new Float32Array(atomCount * 3);
    const radii = new Float32Array(atomCount);
    const colors = new Float32Array(atomCount * 3);
    const indices = new Float32Array(atomCount);
    for (let i = 0; i < atomCount; i++) {
      const p = molecule.positions[i]!;
      centers[i * 3] = p[0] * displayScale;
      centers[i * 3 + 1] = p[1] * displayScale;
      centers[i * 3 + 2] = p[2] * displayScale;
      radii[i] = speciesRadius(molecule, i, representation) * displayScale;
      const c = speciesColor(molecule, i);
      colors[i * 3] = c[0];
      colors[i * 3 + 1] = c[1];
      colors[i * 3 + 2] = c[2];
      indices[i] = i;
    }
    geo.setAttribute("iCenter", new THREE.InstancedBufferAttribute(centers, 3));
    geo.setAttribute("iRadius", new THREE.InstancedBufferAttribute(radii, 1));
    geo.setAttribute("iColor", new THREE.InstancedBufferAttribute(colors, 3));
    geo.setAttribute("iIndex", new THREE.InstancedBufferAttribute(indices, 1));
    geo.instanceCount = atomCount;
    quad.dispose();
    return geo;
  }, [molecule, atomCount, representation, displayScale]);

  const sphereMaterial = useMemo(
    () =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        uniforms: { logDepthBufFC: { value: 1.0 }, uSelected: { value: -1 } },
        vertexShader: SPHERE_VERT,
        fragmentShader: SPHERE_FRAG,
      }),
    [],
  );

  // Invisible real-sphere geometry used only for CPU raycasting (the visible
  // atoms are impostor quads, which the raycaster can't pick). Renders nothing
  // (colorWrite/depthWrite off) but stays present so R3F's onClick works.
  const pickGeom = useMemo(() => new THREE.SphereGeometry(1, 12, 8), []);
  const pickMaterial = useMemo(
    () =>
      new THREE.MeshBasicMaterial({
        colorWrite: false,
        depthWrite: false,
      }),
    [],
  );

  // Bond geometry: two half-cylinders per bond, colored per atom.
  const bondGeom = useMemo(() => new THREE.CylinderGeometry(1, 1, 1, 12, 1), []);
  const bondMaterial = useMemo(
    () =>
      new THREE.ShaderMaterial({
        glslVersion: THREE.GLSL3,
        uniforms: { logDepthBufFC: { value: 1.0 } },
        vertexShader: BOND_VERT,
        fragmentShader: BOND_FRAG,
      }),
    [],
  );

  const bondCount = showBonds ? bonds.length * 2 : 0;

  // The impostor geometry is rebuilt whenever the molecule (or its
  // representation/scale) changes; dispose the superseded GPU buffers so they
  // don't leak across edits.
  useEffect(() => () => sphereGeom.dispose(), [sphereGeom]);

  // Stable-for-the-lifetime resources: release them when the renderer unmounts.
  useEffect(
    () => () => {
      sphereMaterial.dispose();
      pickGeom.dispose();
      pickMaterial.dispose();
      bondGeom.dispose();
      bondMaterial.dispose();
    },
    [sphereMaterial, pickGeom, pickMaterial, bondGeom, bondMaterial],
  );

  useLayoutEffect(() => {
    const inst = bondsRef.current;
    if (!inst || !showBonds) return;
    const radius = (representation === "wireframe" ? 0.05 : 0.11) * displayScale;
    const mat = new THREE.Matrix4();
    const pos = new THREE.Vector3();
    const scale = new THREE.Vector3();
    const quat = new THREE.Quaternion();
    const yAxis = new THREE.Vector3(0, 1, 0);
    const A = new THREE.Vector3();
    const B = new THREE.Vector3();
    const dir = new THREE.Vector3();
    const mid = new THREE.Vector3();
    const color = new THREE.Color();
    let k = 0;
    for (const bond of bonds) {
      const pa = molecule.positions[bond.a]!;
      const pb = molecule.positions[bond.b]!;
      A.set(pa[0] * displayScale, pa[1] * displayScale, pa[2] * displayScale);
      B.set(pb[0] * displayScale, pb[1] * displayScale, pb[2] * displayScale);
      mid.addVectors(A, B).multiplyScalar(0.5);
      dir.subVectors(B, A);
      const len = dir.length();
      if (len < 1e-6) {
        mat.makeScale(0, 0, 0);
        inst.setMatrixAt(k, mat);
        inst.setMatrixAt(k + 1, mat);
        k += 2;
        continue;
      }
      dir.normalize();
      quat.setFromUnitVectors(yAxis, dir);
      // half A: atom a → midpoint
      scale.set(radius, len / 2, radius);
      pos.addVectors(A, mid).multiplyScalar(0.5);
      mat.compose(pos, quat, scale);
      inst.setMatrixAt(k, mat);
      const ca = speciesColor(molecule, bond.a);
      color.setRGB(ca[0], ca[1], ca[2]);
      inst.setColorAt(k, color);
      // half B: midpoint → atom b
      pos.addVectors(mid, B).multiplyScalar(0.5);
      mat.compose(pos, quat, scale);
      inst.setMatrixAt(k + 1, mat);
      const cb = speciesColor(molecule, bond.b);
      color.setRGB(cb[0], cb[1], cb[2]);
      inst.setColorAt(k + 1, color);
      k += 2;
    }
    inst.count = bondCount;
    inst.instanceMatrix.needsUpdate = true;
    if (inst.instanceColor) inst.instanceColor.needsUpdate = true;
  }, [molecule, bonds, showBonds, representation, bondCount, displayScale]);

  // Position the invisible pick spheres to match the visible atoms.
  useLayoutEffect(() => {
    const inst = pickRef.current;
    if (!inst) return;
    const mat = new THREE.Matrix4();
    const pos = new THREE.Vector3();
    const scl = new THREE.Vector3();
    const q = new THREE.Quaternion();
    for (let i = 0; i < atomCount; i++) {
      const p = molecule.positions[i]!;
      const r = speciesRadius(molecule, i, representation) * displayScale;
      pos.set(p[0] * displayScale, p[1] * displayScale, p[2] * displayScale);
      scl.set(r, r, r);
      mat.compose(pos, q, scl);
      inst.setMatrixAt(i, mat);
    }
    inst.count = atomCount;
    inst.instanceMatrix.needsUpdate = true;
  }, [molecule, atomCount, representation, displayScale]);

  // Keep the logarithmic-depth factor and selection highlight in sync.
  useFrame(({ camera }) => {
    const fc = 2.0 / Math.log2((camera as THREE.PerspectiveCamera).far + 1.0);
    sphereMaterial.uniforms.logDepthBufFC!.value = fc;
    bondMaterial.uniforms.logDepthBufFC!.value = fc;
    const sel = useMoleculeStore.getState().selectedAtomIndex;
    sphereMaterial.uniforms.uSelected!.value = sel ?? -1;
  });

  const handlePick = (e: ThreeEvent<MouseEvent>) => {
    if (viewportWasDrag()) return; // ignore clicks that end an orbit/pan
    e.stopPropagation();
    const id = e.instanceId;
    if (id === undefined) return;
    const current = useMoleculeStore.getState().selectedAtomIndex;
    selectAtom(current === id ? null : id); // toggle
  };

  if (atomCount === 0) return null;

  return (
    <group>
      <mesh geometry={sphereGeom} material={sphereMaterial} frustumCulled={false} />
      <instancedMesh
        key={atomCount}
        ref={pickRef}
        args={[pickGeom, pickMaterial, atomCount]}
        frustumCulled={false}
        onClick={handlePick}
      />
      {showBonds && bonds.length > 0 && (
        <instancedMesh
          ref={bondsRef}
          args={[bondGeom, bondMaterial, bondCount]}
          frustumCulled={false}
        />
      )}
    </group>
  );
}
