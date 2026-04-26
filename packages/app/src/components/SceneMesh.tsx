import { memo, useEffect, useRef, useMemo, useCallback, useState } from "react";
import * as THREE from "three";
import { Edges, Html } from "@react-three/drei";
import { useThree } from "@react-three/fiber";
import type { TriangleMesh, PartInfo, FaceInfo } from "@vcad/core";
import { useUiStore, useDocumentStore, useSketchStore, isPcbBoardPart, isStitchPart, isEmbroideryPatternPart } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";
import type { ThreeEvent } from "@react-three/fiber";
import type { Transform3D } from "@vcad/ir";
import { getMaterialByKey } from "@/data/materials";
import { viewportWasDrag } from "@/lib/viewport-drag";
import {
  hasProceduralShader,
  getProceduralShaderForMaterial,
} from "@/shaders";
import { useDebugOverlayStore } from "@/stores/debug-overlay-store";
import { inspectTriangleFromMesh as runInspectTriangle } from "./TriangleInspector";
import { pickSubFeature } from "@/lib/sub-feature-picking";

const HOVER_EMISSIVE = new THREE.Color(0xffb800); // neon amber
const FACE_HIGHLIGHT_COLOR = new THREE.Color(0x00d4ff); // cyan for face selection

const DEG2RAD = Math.PI / 180;
const NORMAL_TOLERANCE = 0.01; // Tolerance for grouping triangles by normal

/** Find all triangle indices that share the same normal as the given triangle */
function findCoplanarTriangles(
  mesh: TriangleMesh,
  targetFaceIndex: number,
): number[] {
  const indices = mesh.indices;
  const positions = mesh.positions;

  // Get the normal of the target triangle
  const ti0 = indices[targetFaceIndex * 3]!;
  const ti1 = indices[targetFaceIndex * 3 + 1]!;
  const ti2 = indices[targetFaceIndex * 3 + 2]!;

  const tv0 = new THREE.Vector3(
    positions[ti0 * 3]!,
    positions[ti0 * 3 + 1]!,
    positions[ti0 * 3 + 2]!,
  );
  const tv1 = new THREE.Vector3(
    positions[ti1 * 3]!,
    positions[ti1 * 3 + 1]!,
    positions[ti1 * 3 + 2]!,
  );
  const tv2 = new THREE.Vector3(
    positions[ti2 * 3]!,
    positions[ti2 * 3 + 1]!,
    positions[ti2 * 3 + 2]!,
  );

  const targetEdge1 = tv1.clone().sub(tv0);
  const targetEdge2 = tv2.clone().sub(tv0);
  const targetNormal = targetEdge1.cross(targetEdge2).normalize();

  // Find all triangles with matching normal
  const matchingTriangles: number[] = [];
  const numTriangles = indices.length / 3;

  for (let i = 0; i < numTriangles; i++) {
    const i0 = indices[i * 3]!;
    const i1 = indices[i * 3 + 1]!;
    const i2 = indices[i * 3 + 2]!;

    const v0 = new THREE.Vector3(
      positions[i0 * 3]!,
      positions[i0 * 3 + 1]!,
      positions[i0 * 3 + 2]!,
    );
    const v1 = new THREE.Vector3(
      positions[i1 * 3]!,
      positions[i1 * 3 + 1]!,
      positions[i1 * 3 + 2]!,
    );
    const v2 = new THREE.Vector3(
      positions[i2 * 3]!,
      positions[i2 * 3 + 1]!,
      positions[i2 * 3 + 2]!,
    );

    const edge1 = v1.clone().sub(v0);
    const edge2 = v2.clone().sub(v0);
    const normal = edge1.cross(edge2).normalize();

    // Check if normals match (dot product close to 1)
    if (normal.dot(targetNormal) > 1 - NORMAL_TOLERANCE) {
      matchingTriangles.push(i);
    }
  }

  return matchingTriangles;
}

/** Build geometry for a subset of triangles */
function buildFaceHighlightGeometry(
  mesh: TriangleMesh,
  triangleIndices: number[],
): THREE.BufferGeometry {
  const geo = new THREE.BufferGeometry();
  const positions: number[] = [];

  for (const triIdx of triangleIndices) {
    const i0 = mesh.indices[triIdx * 3]!;
    const i1 = mesh.indices[triIdx * 3 + 1]!;
    const i2 = mesh.indices[triIdx * 3 + 2]!;

    positions.push(
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

  geo.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
  geo.computeVertexNormals();
  return geo;
}

interface SceneMeshProps {
  partInfo: PartInfo;
  mesh: TriangleMesh;
  materialKey: string;
  selected: boolean;
  transform?: Transform3D;
  /** Override ID used for selection (e.g., instance ID instead of part ID) */
  selectionId?: string;
}

/** Compute face info from a raycast hit.
 *
 * Walks the mesh once to collect all triangles coplanar with the hit
 * triangle, returning the true face centroid (average of unique vertices)
 * and the list of unique vertices on the face. Both are in kernel (Z-up)
 * coordinates, matching the buffer positions — the raycast `hitPoint`
 * passed in by R3F is in world (Y-up) space and is intentionally ignored.
 */
function computeFaceInfo(
  mesh: TriangleMesh,
  faceIndex: number,
  partId: string,
): FaceInfo {
  // Get the hit triangle's normal (kernel space — buffer is unrotated)
  const i0 = mesh.indices[faceIndex * 3]!;
  const i1 = mesh.indices[faceIndex * 3 + 1]!;
  const i2 = mesh.indices[faceIndex * 3 + 2]!;
  const v0 = new THREE.Vector3(
    mesh.positions[i0 * 3]!,
    mesh.positions[i0 * 3 + 1]!,
    mesh.positions[i0 * 3 + 2]!,
  );
  const v1 = new THREE.Vector3(
    mesh.positions[i1 * 3]!,
    mesh.positions[i1 * 3 + 1]!,
    mesh.positions[i1 * 3 + 2]!,
  );
  const v2 = new THREE.Vector3(
    mesh.positions[i2 * 3]!,
    mesh.positions[i2 * 3 + 1]!,
    mesh.positions[i2 * 3 + 2]!,
  );
  const refNormal = v1.clone().sub(v0).cross(v2.clone().sub(v0)).normalize();
  const refOffset = refNormal.dot(v0); // signed distance of plane from origin

  // Walk every triangle, keep those coplanar with the hit triangle, and
  // collect their unique vertices.
  const PLANE_TOLERANCE = 0.01; // mm
  const vertexMap = new Map<string, { x: number; y: number; z: number }>();
  const triCount = mesh.indices.length / 3;
  for (let t = 0; t < triCount; t++) {
    const a = mesh.indices[t * 3]!;
    const b = mesh.indices[t * 3 + 1]!;
    const c = mesh.indices[t * 3 + 2]!;
    const va = new THREE.Vector3(
      mesh.positions[a * 3]!,
      mesh.positions[a * 3 + 1]!,
      mesh.positions[a * 3 + 2]!,
    );
    const vb = new THREE.Vector3(
      mesh.positions[b * 3]!,
      mesh.positions[b * 3 + 1]!,
      mesh.positions[b * 3 + 2]!,
    );
    const vc = new THREE.Vector3(
      mesh.positions[c * 3]!,
      mesh.positions[c * 3 + 1]!,
      mesh.positions[c * 3 + 2]!,
    );
    const triNormal = vb.clone().sub(va).cross(vc.clone().sub(va)).normalize();
    if (triNormal.dot(refNormal) < 1 - NORMAL_TOLERANCE) continue;
    if (Math.abs(refNormal.dot(va) - refOffset) > PLANE_TOLERANCE) continue;

    for (const v of [va, vb, vc]) {
      const key = `${v.x.toFixed(4)},${v.y.toFixed(4)},${v.z.toFixed(4)}`;
      if (!vertexMap.has(key)) {
        vertexMap.set(key, { x: v.x, y: v.y, z: v.z });
      }
    }
  }

  const vertices = Array.from(vertexMap.values());
  // True face centroid: average of unique vertices, in kernel coords.
  let cx = 0;
  let cy = 0;
  let cz = 0;
  for (const v of vertices) {
    cx += v.x;
    cy += v.y;
    cz += v.z;
  }
  const n = Math.max(vertices.length, 1);
  const centroid = { x: cx / n, y: cy / n, z: cz / n };

  return {
    partId,
    faceIndex,
    normal: { x: refNormal.x, y: refNormal.y, z: refNormal.z },
    centroid,
    vertices,
  };
}

/**
 * Simplified mesh component for imported files (STL/STEP) that don't have PartInfo.
 * No selection/hover/rename UI - just renders the mesh with material and wireframe toggle.
 */
interface ImportedMeshProps {
  mesh: TriangleMesh;
  materialKey: string;
}

export function ImportedMesh({ mesh, materialKey }: ImportedMeshProps) {
  const geoRef = useRef<THREE.BufferGeometry>(null);
  const meshRef = useRef<THREE.Mesh>(null);
  const [geoReady, setGeoReady] = useState(false);
  const showWireframe = useUiStore((s) => s.showWireframe);
  const isOrbiting = useUiStore((s) => s.isOrbiting);
  const materials = useDocumentStore((s) => s.document.materials);

  // Resolve material from document, falling back to the preset library.
  const materialDef = useMemo(() => {
    if (materials[materialKey]) return materials[materialKey];
    const preset = getMaterialByKey(materialKey);
    if (preset) {
      return {
        name: preset.name,
        color: preset.color,
        metallic: preset.metallic,
        roughness: preset.roughness,
      };
    }
    return null;
  }, [materials, materialKey]);

  const materialColor = useMemo(() => {
    if (materialDef) {
      return new THREE.Color(
        materialDef.color[0],
        materialDef.color[1],
        materialDef.color[2],
      );
    }
    return new THREE.Color(0.55, 0.55, 0.55);
  }, [materialDef]);

  useEffect(() => {
    setGeoReady(false);
    const geo = geoRef.current;
    if (!geo) return;

    // Imported meshes arrive without kernel-emitted normals. Route them
    // through the WASM render-bake pipeline so the attribute layout is
    // identical to everything else the renderer consumes — no three.js-only
    // `toCreasedNormals` dependency, no fallback path that silently
    // disagrees with the primary render pipeline.
    let disposed = false;
    (async () => {
      const { getKernelWasmSync } = await import("@vcad/engine");
      const kernel = getKernelWasmSync() as unknown as {
        renderBakeMesh?: (inputJson: string) => string;
      } | null;
      if (disposed) return;

      const positions = new Float32Array(mesh.positions);
      const indices = new Uint32Array(mesh.indices);
      let bakedPositions: Float32Array;
      let bakedIndices: Uint32Array;
      let bakedNormals: Float32Array;

      if (kernel?.renderBakeMesh) {
        const out = JSON.parse(
          kernel.renderBakeMesh(
            JSON.stringify({
              positions: Array.from(positions),
              indices: Array.from(indices),
            }),
          ),
        ) as { positions: number[]; indices: number[]; normals: number[] };
        bakedPositions = new Float32Array(out.positions);
        bakedIndices = new Uint32Array(out.indices);
        bakedNormals = new Float32Array(out.normals);
      } else {
        // Kernel WASM not booted yet — fall back to bare positions.
        // The consumer will retriangulate next effect tick.
        bakedPositions = positions;
        bakedIndices = indices;
        bakedNormals = new Float32Array(0);
      }

      geo.setAttribute("position", new THREE.BufferAttribute(bakedPositions, 3));
      geo.setIndex(new THREE.BufferAttribute(bakedIndices, 1));
      if (bakedNormals.length === bakedPositions.length) {
        geo.setAttribute("normal", new THREE.BufferAttribute(bakedNormals, 3));
      } else {
        geo.computeVertexNormals();
      }
      geo.computeBoundingSphere();
      geo.computeBoundingBox();
      setGeoReady(true);
    })();

    return () => {
      disposed = true;
      geo.dispose();
    };
  }, [mesh]);

  // Disable raycasting during orbit for performance
  const originalRaycastRef = useRef<THREE.Mesh["raycast"] | null>(null);
  useEffect(() => {
    const m = meshRef.current;
    if (!m) return;

    if (!originalRaycastRef.current) {
      originalRaycastRef.current = m.raycast.bind(m);
    }

    if (isOrbiting) {
      m.raycast = () => {};
    } else {
      m.raycast = originalRaycastRef.current;
    }
  }, [isOrbiting]);

  return (
    <mesh ref={meshRef} castShadow receiveShadow>
      <bufferGeometry ref={geoRef} />
      <meshStandardMaterial
        color={materialColor}
        metalness={materialDef?.metallic ?? 0.0}
        roughness={materialDef?.roughness ?? 0.7}
        envMapIntensity={1.0}
        flatShading={false}
        side={THREE.DoubleSide}
      />
      {showWireframe && geoReady && <Edges threshold={15} color="#666" />}
    </mesh>
  );
}

export const SceneMesh = memo(function SceneMesh({
  partInfo,
  mesh,
  materialKey,
  selected,
  transform,
  selectionId,
}: SceneMeshProps) {
  const geoRef = useRef<THREE.BufferGeometry>(null);
  const meshRef = useRef<THREE.Mesh>(null);
  const [geoReady, setGeoReady] = useState(false);
  const select = useUiStore((s) => s.select);
  const toggleSelect = useUiStore((s) => s.toggleSelect);
  const selectItem = useUiStore((s) => s.selectItem);
  const toggleItem = useUiStore((s) => s.toggleItem);
  const setHoveredItem = useUiStore((s) => s.setHoveredItem);
  const selectionFilter = useUiStore((s) => s.selectionFilter);
  const showWireframe = useUiStore((s) => s.showWireframe);
  const hoveredPartId = useUiStore((s) => s.hoveredPartId);
  const setHoveredPartId = useUiStore((s) => s.setHoveredPartId);
  const camera = useThree((s) => s.camera);
  const viewportSize = useThree((s) => s.size);
  const materials = useDocumentStore((s) => s.document.materials);
  const renamePart = useDocumentStore((s) => s.renamePart);

  // Face selection state
  const faceSelectionMode = useSketchStore((s) => s.faceSelectionMode);
  const hoveredFace = useSketchStore((s) => s.hoveredFace);
  const setHoveredFace = useSketchStore((s) => s.setHoveredFace);
  const selectFace = useSketchStore((s) => s.selectFace);

  // Disable raycasting during orbit for performance
  const isOrbiting = useUiStore((s) => s.isOrbiting);
  // During AI screenshot capture, suppress emissive tint so user-selected
  // parts don't glow in the shot — the AI is verifying geometry/materials,
  // not watching the user's cursor.
  const captureMode = useUiStore((s) => s.captureMode);
  const effectiveSelected = selected && !captureMode;

  // Use selectionId if provided, otherwise fall back to partInfo.id
  const effectiveSelectionId = selectionId ?? partInfo.id;
  const isHovered = hoveredPartId === effectiveSelectionId;
  const isHoveredFace =
    faceSelectionMode && hoveredFace?.partId === partInfo.id;

  // Compute highlighted face geometry (triangles sharing same normal)
  // Skip computation during orbit for performance
  const faceHighlightGeo = useMemo(() => {
    if (isOrbiting || !isHoveredFace || hoveredFace?.faceIndex == null) return null;
    const matchingTriangles = findCoplanarTriangles(
      mesh,
      hoveredFace.faceIndex,
    );
    return buildFaceHighlightGeometry(mesh, matchingTriangles);
  }, [isOrbiting, isHoveredFace, hoveredFace?.faceIndex, mesh]);

  // Cleanup face highlight geometry
  useEffect(() => {
    return () => {
      faceHighlightGeo?.dispose();
    };
  }, [faceHighlightGeo]);
  const [isRenaming, setIsRenaming] = useState(false);
  const [draftName, setDraftName] = useState(partInfo.name);
  const nameInputRef = useRef<HTMLInputElement>(null);

  // Material preview state for live preview on hover
  const previewMaterial = useUiStore((s) => s.previewMaterial);

  // Determine effective material key (preview takes priority)
  const effectiveMaterialKey = useMemo(() => {
    if (previewMaterial?.partId === partInfo.id) {
      return previewMaterial.materialKey;
    }
    return materialKey;
  }, [previewMaterial, partInfo.id, materialKey]);

  // Resolve material from document, with live preview override
  const materialDef = useMemo(() => {
    // Check for active preview for this part
    if (previewMaterial?.partId === partInfo.id) {
      const previewKey = previewMaterial.materialKey;
      // First check document materials
      if (materials[previewKey]) {
        return materials[previewKey];
      }
      // Fall back to preset materials library
      const preset = getMaterialByKey(previewKey);
      if (preset) {
        return {
          name: preset.name,
          color: preset.color,
          metallic: preset.metallic,
          roughness: preset.roughness,
        };
      }
    }
    if (materials[materialKey]) return materials[materialKey];
    const preset = getMaterialByKey(materialKey);
    if (preset) {
      return {
        name: preset.name,
        color: preset.color,
        metallic: preset.metallic,
        roughness: preset.roughness,
      };
    }
    return null;
  }, [materials, materialKey, previewMaterial, partInfo.id]);

  // Check if this material should use a procedural shader
  const proceduralShader = useMemo(() => {
    if (!hasProceduralShader(effectiveMaterialKey)) return null;
    return getProceduralShaderForMaterial(effectiveMaterialKey);
  }, [effectiveMaterialKey]);

  // Create procedural ShaderMaterial if needed
  const shaderMaterial = useMemo(() => {
    if (!proceduralShader) return null;

    const mat = new THREE.ShaderMaterial({
      vertexShader: proceduralShader.vertexShader,
      fragmentShader: proceduralShader.fragmentShader,
      uniforms: proceduralShader.uniforms,
      side: THREE.DoubleSide,
    });

    return mat;
  }, [proceduralShader]);

  // Cleanup shader material
  useEffect(() => {
    return () => {
      shaderMaterial?.dispose();
    };
  }, [shaderMaterial]);

  const materialColor = useMemo(() => {
    if (materialDef) {
      return new THREE.Color(
        materialDef.color[0],
        materialDef.color[1],
        materialDef.color[2],
      );
    }
    return new THREE.Color(0.55, 0.55, 0.55);
  }, [materialDef]);

  // Compute emissive state: selected > hovered > none (face highlight uses overlay)
  const emissiveColor = useMemo(() => {
    if (effectiveSelected) return materialColor.clone().multiplyScalar(0.3);
    if (isHovered && !faceSelectionMode && !captureMode) return HOVER_EMISSIVE;
    return undefined;
  }, [effectiveSelected, isHovered, faceSelectionMode, captureMode, materialColor]);

  const emissiveIntensity = effectiveSelected
    ? 0.2
    : isHovered && !faceSelectionMode && !captureMode
    ? 0.08
    : 0;

  // Update shader material uniforms for emissive state
  useEffect(() => {
    if (!shaderMaterial) return;
    const uniforms = shaderMaterial.uniforms;
    if (!uniforms["uEmissive"] || !uniforms["uEmissiveIntensity"]) return;

    if (effectiveSelected) {
      uniforms["uEmissive"].value = materialColor.clone().multiplyScalar(0.3);
      uniforms["uEmissiveIntensity"].value = 0.2;
    } else if (isHovered && !faceSelectionMode && !captureMode) {
      uniforms["uEmissive"].value = HOVER_EMISSIVE;
      uniforms["uEmissiveIntensity"].value = 0.08;
    } else {
      uniforms["uEmissive"].value = new THREE.Color(0, 0, 0);
      uniforms["uEmissiveIntensity"].value = 0;
    }
  }, [shaderMaterial, effectiveSelected, isHovered, faceSelectionMode, captureMode, materialColor]);

  useEffect(() => {
    setDraftName(partInfo.name);
  }, [partInfo.name, selected]);

  useEffect(() => {
    if (isRenaming) {
      nameInputRef.current?.select();
    }
  }, [isRenaming]);

  useEffect(() => {
    setGeoReady(false);
    const geo = geoRef.current;
    if (!geo) return;

    // Clone arrays to avoid issues with transferred/shared buffers
    const positions = new Float32Array(mesh.positions);
    const indices = new Uint32Array(mesh.indices);

    geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geo.setIndex(new THREE.BufferAttribute(indices, 1));

    // Kernel emits crease-aware vertex normals as part of the render-ready
    // mesh (see `apply_default_creased_normals` in vcad-kernel-tessellate).
    // Every Solid.getMesh() path carries them, so this attribute is always
    // present. The fallback `computeVertexNormals` path was removed so
    // shading stays identical across primitives, extrudes, revolves, and
    // any future renderer.
    if (mesh.normals && mesh.normals.length === positions.length) {
      const normals = new Float32Array(mesh.normals);
      geo.setAttribute("normal", new THREE.BufferAttribute(normals, 3));
    } else {
      console.warn(
        "[SceneMesh] kernel emitted mesh without normals — rebuild @vcad/kernel-wasm; falling back to computed normals",
      );
      geo.computeVertexNormals();
    }

    // Per-vertex colors (e.g. embroidery thread colors)
    if (mesh.colors && mesh.colors.length === positions.length) {
      const colors = new Float32Array(mesh.colors);
      geo.setAttribute("color", new THREE.BufferAttribute(colors, 3));
    } else {
      geo.deleteAttribute("color");
    }

    geo.computeBoundingSphere();
    geo.computeBoundingBox();
    setGeoReady(true);

    return () => {
      geo.dispose();
    };
  }, [mesh, partInfo.name]);

  // Apply Transform3D to mesh (for assembly instances)
  useEffect(() => {
    const m = meshRef.current;
    if (!m) return;

    if (transform) {
      m.position.set(
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
      );
      const euler = new THREE.Euler(
        transform.rotation.x * DEG2RAD,
        transform.rotation.y * DEG2RAD,
        transform.rotation.z * DEG2RAD,
        "XYZ",
      );
      m.quaternion.setFromEuler(euler);
      m.scale.set(transform.scale.x, transform.scale.y, transform.scale.z);
    } else {
      // Reset to identity if no transform
      m.position.set(0, 0, 0);
      m.quaternion.identity();
      m.scale.set(1, 1, 1);
    }
  }, [transform]);

  // Compute name tag position above the part
  const labelPosition = useMemo(() => {
    if (!mesh.positions.length) return new THREE.Vector3();
    const box = new THREE.Box3();
    const pos = new THREE.Vector3();
    for (let i = 0; i < mesh.positions.length; i += 3) {
      pos.set(
        mesh.positions[i]!,
        mesh.positions[i + 1]!,
        mesh.positions[i + 2]!,
      );
      box.expandByPoint(pos);
    }
    const topCenter = new THREE.Vector3();
    box.getCenter(topCenter);
    topCenter.z = box.max.z + 4;
    return topCenter;
  }, [mesh.positions]);

  const commitRename = useCallback(() => {
    const trimmed = draftName.trim();
    if (trimmed && trimmed !== partInfo.name) {
      renamePart(partInfo.id, trimmed);
    }
    setIsRenaming(false);
  }, [draftName, partInfo.id, partInfo.name, renamePart]);

  const cancelRename = useCallback(() => {
    setDraftName(partInfo.name);
    setIsRenaming(false);
  }, [partInfo.name]);

  const inspectTriangles = useDebugOverlayStore((s) => s.inspectTriangles);
  const setCurrentInspection = useDebugOverlayStore((s) => s.setCurrentInspection);

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      // Ignore the click that fires after a camera rotate/pan gesture.
      if (viewportWasDrag()) return;
      e.stopPropagation();

      // Debug triangle inspector: when enabled, show triangle info.
      if (inspectTriangles && e.faceIndex != null) {
        const result = runInspectTriangle(mesh, e.faceIndex);
        if (result) {
          setCurrentInspection(result);
          // eslint-disable-next-line no-console
          console.log("[triangle-inspector]", result);
        }
        return;
      }

      // In face selection mode, select the face
      if (faceSelectionMode && e.faceIndex != null) {
        const faceInfo = computeFaceInfo(mesh, e.faceIndex, partInfo.id);
        selectFace(faceInfo);
        return;
      }

      // Sub-feature picker: vertex / edge / face / body, gated by the
      // current selectionFilter. Falls through to body-only behavior when
      // the filter is "auto" and there's no candidate within threshold,
      // or when the filter is explicitly "body".
      if (e.faceIndex != null) {
        const item = pickSubFeature({
          triIndex: e.faceIndex,
          hitPoint: e.point,
          mesh,
          partId: partInfo.id,
          filter: selectionFilter,
          camera,
          viewport: viewportSize,
        });
        if (item) {
          if (e.nativeEvent.shiftKey) {
            toggleItem(item);
          } else {
            selectItem(item);
          }
          return;
        }
      }

      // Filter narrowed too far / fallback to part-level select.
      if (e.nativeEvent.shiftKey) {
        toggleSelect(partInfo.id);
      } else {
        select(partInfo.id);
      }
    },
    [
      faceSelectionMode,
      mesh,
      partInfo.id,
      selectFace,
      toggleSelect,
      select,
      selectItem,
      toggleItem,
      selectionFilter,
      camera,
      viewportSize,
      inspectTriangles,
      setCurrentInspection,
    ],
  );

  const handlePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      // Skip during orbit for performance
      if (isOrbiting) return;
      if (faceSelectionMode && e.faceIndex != null) {
        e.stopPropagation();
        const faceInfo = computeFaceInfo(mesh, e.faceIndex, partInfo.id);
        setHoveredFace(faceInfo);
        return;
      }
      // Outside face-selection mode, drive the unified hovered item via
      // the sub-feature picker. The filter restricts what kinds get
      // picked; "auto" prefers vertex > edge > face. Falls back to body.
      if (e.faceIndex != null) {
        e.stopPropagation();
        const item = pickSubFeature({
          triIndex: e.faceIndex,
          hitPoint: e.point,
          mesh,
          partId: partInfo.id,
          filter: selectionFilter,
          camera,
          viewport: viewportSize,
        });
        if (item) {
          setHoveredItem(item);
        } else {
          setHoveredItem({ kind: "part", id: partInfo.id });
        }
      }
    },
    [
      isOrbiting,
      faceSelectionMode,
      mesh,
      partInfo.id,
      setHoveredFace,
      setHoveredItem,
      selectionFilter,
      camera,
      viewportSize,
    ],
  );

  const handlePointerOver = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      // Skip during orbit for performance
      if (isOrbiting) return;
      e.stopPropagation();
      if (!faceSelectionMode) {
        setHoveredPartId(partInfo.id);
      }
    },
    [isOrbiting, faceSelectionMode, partInfo.id, setHoveredPartId],
  );

  const handlePointerOut = useCallback(() => {
    // Skip during orbit for performance
    if (isOrbiting) return;
    if (faceSelectionMode) {
      setHoveredFace(null);
    } else {
      setHoveredPartId(null);
    }
  }, [isOrbiting, faceSelectionMode, setHoveredFace, setHoveredPartId]);

  // Store original raycast function and disable during orbit for performance
  const originalRaycastRef = useRef<THREE.Mesh["raycast"] | null>(null);
  useEffect(() => {
    const m = meshRef.current;
    if (!m) return;

    // Store original raycast function on first mount
    if (!originalRaycastRef.current) {
      originalRaycastRef.current = m.raycast.bind(m);
    }

    if (isOrbiting) {
      // Disable raycasting during orbit
      m.raycast = () => {};
    } else {
      // Restore original raycast
      m.raycast = originalRaycastRef.current;
    }
  }, [isOrbiting]);

  return (
    <mesh
      ref={meshRef}
      castShadow
      receiveShadow
      onClick={handleClick}
      onPointerMove={handlePointerMove}
      onPointerOver={handlePointerOver}
      onPointerOut={handlePointerOut}
      material={shaderMaterial ?? undefined}
    >
      <bufferGeometry ref={geoRef} />
      {/* Use procedural shader if available, otherwise standard PBR */}
      {!shaderMaterial && (
        <meshStandardMaterial
          color={mesh.colors ? undefined : materialColor}
          vertexColors={!!mesh.colors}
          emissive={emissiveColor}
          emissiveIntensity={emissiveIntensity}
          metalness={materialDef?.metallic ?? 0.0}
          roughness={materialDef?.roughness ?? 0.7}
          envMapIntensity={1.0}
          flatShading={false}
          side={THREE.DoubleSide}
        />
      )}
      {showWireframe && geoReady && <Edges threshold={15} color="#666" />}
      {/* Face highlight overlay for individual face selection */}
      {faceHighlightGeo && (
        <mesh geometry={faceHighlightGeo} renderOrder={1}>
          <meshBasicMaterial
            color={FACE_HIGHLIGHT_COLOR}
            transparent
            opacity={0.4}
            depthTest={true}
            depthWrite={false}
            polygonOffset={true}
            polygonOffsetFactor={-4}
            polygonOffsetUnits={-4}
            side={THREE.DoubleSide}
          />
        </mesh>
      )}
      {selected && !faceSelectionMode && (
        <Html position={labelPosition} center style={{ pointerEvents: "auto" }}>
          <div className="flex items-center gap-1.5 px-2 py-1 text-xs font-medium text-text whitespace-nowrap bg-surface/90 backdrop-blur-sm border border-border rounded-md shadow-sm">
            {isRenaming ? (
              <input
                ref={nameInputRef}
                type="text"
                value={draftName}
                onClick={(e) => e.stopPropagation()}
                onFocus={(e) => e.currentTarget.select()}
                onChange={(e) => setDraftName(e.target.value)}
                onBlur={commitRename}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitRename();
                  if (e.key === "Escape") cancelRename();
                }}
                className="min-w-[80px] bg-transparent text-text outline-none"
                autoFocus
              />
            ) : (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  setIsRenaming(true);
                }}
                className="text-text"
              >
                {partInfo.name}
              </button>
            )}
            {!isRenaming && isPcbBoardPart(partInfo) && (
              <button
                type="button"
                onClick={(e) => { e.stopPropagation(); useElectronicsStore.getState().enter(); }}
                className="px-1.5 py-0.5 text-[10px] rounded bg-brand/15 text-brand hover:bg-brand/25 transition-colors"
              >
                Edit
              </button>
            )}
            {!isRenaming && (isStitchPart(partInfo) || isEmbroideryPatternPart(partInfo)) && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  useEmbroideryStore.getState().openPanel();
                }}
                className="px-1.5 py-0.5 text-[10px] rounded bg-brand/15 text-brand hover:bg-brand/25 transition-colors"
              >
                Edit
              </button>
            )}
          </div>
        </Html>
      )}
    </mesh>
  );
});
