/**
 * Click-to-inspect triangle picker.
 *
 * With the "inspect triangles" debug flag on (Ctrl+Shift+T), clicking
 * any triangle in the viewport populates `currentInspection` in the
 * debug overlay store with:
 *
 *   - Triangle index and source face kind (Cylinder/Plane/Torus/
 *     Sphere/FanFill/Unknown) from the kernel's `face_kinds` tag
 *   - Three vertex positions (kernel coords, Z-up)
 *   - CCW face normal from winding
 *   - Dot product with the mesh-centroid → triangle-centroid vector
 *     (positive = outward-facing, negative = INWARD-wound)
 *
 * A DOM panel renders the result; an R3F marker highlights the picked
 * triangle in the scene.
 */
import { useMemo } from "react";
import * as THREE from "three";
import type { TriangleMesh } from "@vcad/core";
import {
  useDebugOverlayStore,
  type InspectedTriangle,
} from "../stores/debug-overlay-store";

export const FACE_KIND_NAMES = [
  "Unknown",
  "Plane",
  "Cylinder",
  "Sphere",
  "Cone",
  "Bilinear",
  "Torus",
  "BSpline",
  "FanFill",
];

export const FACE_KIND_COLORS = [
  "#888888", // Unknown
  "#ffcc00", // Plane
  "#66cc66", // Cylinder
  "#ff66ff", // Sphere
  "#ff8800", // Cone
  "#88ccff", // Bilinear
  "#00ccff", // Torus
  "#aa66ff", // BSpline
  "#ff3366", // FanFill
];

export function inspectTriangleFromMesh(
  mesh: TriangleMesh,
  triangleIndex: number,
): InspectedTriangle | null {
  const { positions, indices, faceKinds } = mesh;
  if (triangleIndex < 0 || triangleIndex * 3 + 2 >= indices.length) return null;

  const a = indices[triangleIndex * 3]!;
  const b = indices[triangleIndex * 3 + 1]!;
  const c = indices[triangleIndex * 3 + 2]!;

  const pa: [number, number, number] = [
    positions[a * 3]!,
    positions[a * 3 + 1]!,
    positions[a * 3 + 2]!,
  ];
  const pb: [number, number, number] = [
    positions[b * 3]!,
    positions[b * 3 + 1]!,
    positions[b * 3 + 2]!,
  ];
  const pc: [number, number, number] = [
    positions[c * 3]!,
    positions[c * 3 + 1]!,
    positions[c * 3 + 2]!,
  ];
  const centroid: [number, number, number] = [
    (pa[0] + pb[0] + pc[0]) / 3,
    (pa[1] + pb[1] + pc[1]) / 3,
    (pa[2] + pb[2] + pc[2]) / 3,
  ];

  const ex = pb[0] - pa[0];
  const ey = pb[1] - pa[1];
  const ez = pb[2] - pa[2];
  const fx = pc[0] - pa[0];
  const fy = pc[1] - pa[1];
  const fz = pc[2] - pa[2];
  let nx = ey * fz - ez * fy;
  let ny = ez * fx - ex * fz;
  let nz = ex * fy - ey * fx;
  const nmag = Math.sqrt(nx * nx + ny * ny + nz * nz);
  if (nmag > 0) {
    nx /= nmag;
    ny /= nmag;
    nz /= nmag;
  }

  let mcx = 0, mcy = 0, mcz = 0;
  const nVerts = positions.length / 3;
  for (let i = 0; i < nVerts; i++) {
    mcx += positions[i * 3]!;
    mcy += positions[i * 3 + 1]!;
    mcz += positions[i * 3 + 2]!;
  }
  if (nVerts > 0) {
    mcx /= nVerts;
    mcy /= nVerts;
    mcz /= nVerts;
  }
  const ox = centroid[0] - mcx;
  const oy = centroid[1] - mcy;
  const oz = centroid[2] - mcz;
  const omag = Math.sqrt(ox * ox + oy * oy + oz * oz) || 1;
  const outwardDot = (nx * ox + ny * oy + nz * oz) / omag;

  const faceKind = faceKinds?.[triangleIndex] ?? 0;
  const faceKindName = FACE_KIND_NAMES[faceKind] ?? `kind(${faceKind})`;

  return {
    triangleIndex,
    faceKind,
    faceKindName,
    vertexIds: [a, b, c],
    positions: [pa, pb, pc],
    centroid,
    ccwNormal: [nx, ny, nz],
    outwardDot,
  };
}


/** DOM-level panel showing the current inspection (positioned absolutely). */
export function TriangleInspectionPanel() {
  const inspection = useDebugOverlayStore((s) => s.currentInspection);
  const setCurrentInspection = useDebugOverlayStore((s) => s.setCurrentInspection);
  const inspectEnabled = useDebugOverlayStore((s) => s.inspectTriangles);
  if (!inspectEnabled || !inspection) return null;
  const color = FACE_KIND_COLORS[inspection.faceKind] ?? "#888";
  const { outwardDot } = inspection;
  const normalLabel =
    outwardDot > 0.5
      ? "outward ✓"
      : outwardDot < -0.5
        ? "INWARD ✗"
        : "sideways";
  return (
    <div
      style={{
        position: "absolute",
        top: 60,
        left: 12,
        background: "rgba(20,20,24,0.92)",
        color: "#eee",
        fontFamily: "monospace",
        fontSize: 12,
        padding: "10px 14px",
        border: `1px solid ${color}`,
        borderRadius: 4,
        zIndex: 50,
        maxWidth: 360,
        lineHeight: 1.5,
        pointerEvents: "auto",
      }}
    >
      <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 6 }}>
        <div>
          <span
            style={{
              display: "inline-block",
              width: 10,
              height: 10,
              background: color,
              marginRight: 6,
              verticalAlign: "middle",
            }}
          />
          <strong>{inspection.faceKindName}</strong>
          <span style={{ color: "#888", marginLeft: 8 }}>
            tri #{inspection.triangleIndex}
          </span>
        </div>
        <button
          onClick={() => setCurrentInspection(null)}
          style={{
            background: "none",
            border: "none",
            color: "#888",
            cursor: "pointer",
            fontSize: 12,
          }}
        >
          ✕
        </button>
      </div>
      <div style={{ color: "#aaa" }}>
        verts: {inspection.vertexIds.map((v) => `#${v}`).join("  ")}
      </div>
      {inspection.positions.map((p, i) => (
        <div key={i} style={{ color: "#ccc" }}>
          {" "}({p[0].toFixed(3)}, {p[1].toFixed(3)}, {p[2].toFixed(3)})
        </div>
      ))}
      <div style={{ marginTop: 6, color: "#aaa" }}>
        CCW normal: ({inspection.ccwNormal[0].toFixed(3)}, {inspection.ccwNormal[1].toFixed(3)}, {inspection.ccwNormal[2].toFixed(3)})
      </div>
      <div style={{ color: outwardDot < 0 ? "#ff6666" : "#66ff66" }}>
        outward dot: {outwardDot.toFixed(3)} — {normalLabel}
      </div>
    </div>
  );
}

/** R3F marker: render a single filled triangle on top of the picked one. */
export function InspectedTriangleMarker() {
  const inspection = useDebugOverlayStore((s) => s.currentInspection);
  const geometry = useMemo(() => {
    if (!inspection) return null;
    const geo = new THREE.BufferGeometry();
    const pos = new Float32Array([
      ...inspection.positions[0],
      ...inspection.positions[1],
      ...inspection.positions[2],
    ]);
    geo.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    geo.setIndex([0, 1, 2]);
    return geo;
  }, [inspection]);
  if (!inspection || !geometry) return null;
  const color = FACE_KIND_COLORS[inspection.faceKind] ?? "#ffffff";
  return (
    <mesh geometry={geometry} renderOrder={999}>
      <meshBasicMaterial
        color={color}
        transparent
        opacity={0.5}
        depthTest={false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}
