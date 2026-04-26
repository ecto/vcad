import { useRef, useMemo, useCallback, type ReactNode } from "react";
import * as THREE from "three";
import { useThree, useFrame, ThreeEvent } from "@react-three/fiber";
import { Line, Html } from "@react-three/drei";
import {
  useSketchStore,
  useUiStore,
  getPlaneBasis,
  hitTestSegments,
  snapPoint,
} from "@vcad/core";
import type { Vec2, Vec3, SketchConstraint, SketchSegment2D } from "@vcad/ir";
import { useTheme } from "@/hooks/useTheme";
import { viewportWasDrag } from "@/lib/viewport-drag";

const GRID_SIZE = 10; // mm
const GRID_EXTENT = 200; // mm from origin
const POINT_SNAP_TOLERANCE = 5; // mm

/** Convert a Vec3 to Three.js Vector3 */
function toVec3(v: Vec3): THREE.Vector3 {
  return new THREE.Vector3(v.x, v.y, v.z);
}

/**
 * Wraps children in a group that maintains a fixed pixel size as the camera
 * moves. Pass `screenPx=1` and use unit-pixel-sized geometry — e.g.,
 * `<ringGeometry args={[8, 10, 16]} />` will render as 8-10 pixels regardless
 * of zoom.
 *
 * For perspective cameras: world-units per screen pixel is
 *   (2 · distance · tan(fov / 2)) / viewport-height
 * we apply that as the group's uniform scale every frame.
 */
function ScreenScaledGroup({
  position,
  screenPx = 1,
  children,
}: {
  position: THREE.Vector3 | [number, number, number];
  screenPx?: number;
  children: ReactNode;
}) {
  const ref = useRef<THREE.Group>(null);
  const { camera, size } = useThree();
  const tmpV = useRef(new THREE.Vector3());

  useFrame(() => {
    const g = ref.current;
    if (!g) return;
    const p = Array.isArray(position) ? tmpV.current.fromArray(position) : position;
    const cam = camera as THREE.PerspectiveCamera;
    const dist = cam.position.distanceTo(p);
    const fovRad = ((cam.fov ?? 50) * Math.PI) / 180;
    const worldPerPx = (2 * dist * Math.tan(fovRad / 2)) / size.height;
    g.scale.setScalar(Math.max(1e-4, screenPx * worldPerPx));
  });

  return (
    <group ref={ref} position={position}>
      {children}
    </group>
  );
}

/**
 * Inlined 2D → 3D projection helper — the math is trivial once the
 * basis is known, and this runs on every pointer event so we don't
 * want the JSON round-trip through WASM. Use the kernel-backed
 * helpers in `@vcad/core/sketch-math` when you need something less
 * latency-sensitive.
 */
function sketchToWorld(
  pt: Vec2,
  origin: Vec3,
  xDir: Vec3,
  yDir: Vec3,
): THREE.Vector3 {
  return new THREE.Vector3(
    origin.x + pt.x * xDir.x + pt.y * yDir.x,
    origin.y + pt.x * xDir.y + pt.y * yDir.y,
    origin.z + pt.x * xDir.z + pt.y * yDir.z,
  );
}

/** Inlined 3D → 2D projection (see `sketchToWorld` for rationale). */
function worldToSketchFast(
  world: THREE.Vector3,
  origin: Vec3,
  xDir: Vec3,
  yDir: Vec3,
): Vec2 {
  const dx = world.x - origin.x;
  const dy = world.y - origin.y;
  const dz = world.z - origin.z;
  return {
    x: dx * xDir.x + dy * xDir.y + dz * xDir.z,
    y: dx * yDir.x + dy * yDir.y + dz * yDir.z,
  };
}

interface SketchGrid3DProps {
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
  /** 2D bounds in sketch-local (U/V) coordinates. When provided, the grid
   *  is sized to wrap the face being sketched on instead of being a fixed
   *  ±GRID_EXTENT square. Bounds are snapped outward to the nearest cell. */
  bounds: { minU: number; maxU: number; minV: number; maxV: number } | null;
}

/** Grid rendered on the sketch plane */
function SketchGrid3D({ origin, xDir, yDir, bounds }: SketchGrid3DProps) {
  const { isDark } = useTheme();

  // Snap-extend the bounds outward to the nearest GRID_SIZE multiple in
  // sketch-local coordinates. Falls back to a centered ±GRID_EXTENT square
  // when there's no face (sketching on XY/XZ/YZ reference planes).
  const extents = useMemo(() => {
    if (!bounds) {
      return {
        minU: -GRID_EXTENT,
        maxU: GRID_EXTENT,
        minV: -GRID_EXTENT,
        maxV: GRID_EXTENT,
      };
    }
    const pad = GRID_SIZE;
    return {
      minU: Math.floor((bounds.minU - pad) / GRID_SIZE) * GRID_SIZE,
      maxU: Math.ceil((bounds.maxU + pad) / GRID_SIZE) * GRID_SIZE,
      minV: Math.floor((bounds.minV - pad) / GRID_SIZE) * GRID_SIZE,
      maxV: Math.ceil((bounds.maxV + pad) / GRID_SIZE) * GRID_SIZE,
    };
  }, [bounds]);

  // Build grid lines in sketch plane coordinate system
  const gridLines = useMemo(() => {
    const lines: {
      points: [number, number, number][];
      color: string;
      width: number;
    }[] = [];
    const o = toVec3(origin);
    const x = toVec3(xDir);
    const y = toVec3(yDir);
    const color = isDark ? "rgba(255,255,255,0.1)" : "rgba(0,0,0,0.1)";

    // Lines parallel to the U axis (constant V), spanning [minU, maxU]
    for (let v = extents.minV; v <= extents.maxV; v += GRID_SIZE) {
      if (v === 0) continue; // axis is drawn separately
      const start = o
        .clone()
        .add(x.clone().multiplyScalar(extents.minU))
        .add(y.clone().multiplyScalar(v));
      const end = o
        .clone()
        .add(x.clone().multiplyScalar(extents.maxU))
        .add(y.clone().multiplyScalar(v));
      lines.push({
        points: [
          [start.x, start.y, start.z],
          [end.x, end.y, end.z],
        ],
        color,
        width: 1,
      });
    }

    // Lines parallel to the V axis (constant U), spanning [minV, maxV]
    for (let u = extents.minU; u <= extents.maxU; u += GRID_SIZE) {
      if (u === 0) continue;
      const start = o
        .clone()
        .add(y.clone().multiplyScalar(extents.minV))
        .add(x.clone().multiplyScalar(u));
      const end = o
        .clone()
        .add(y.clone().multiplyScalar(extents.maxV))
        .add(x.clone().multiplyScalar(u));
      lines.push({
        points: [
          [start.x, start.y, start.z],
          [end.x, end.y, end.z],
        ],
        color,
        width: 1,
      });
    }

    return lines;
  }, [origin, xDir, yDir, isDark, extents]);

  // Axis lines — span the full grid extent.
  const xAxisPoints = useMemo(() => {
    const o = toVec3(origin);
    const x = toVec3(xDir);
    const start = o.clone().add(x.clone().multiplyScalar(extents.minU));
    const end = o.clone().add(x.clone().multiplyScalar(extents.maxU));
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [origin, xDir, extents]);

  const yAxisPoints = useMemo(() => {
    const o = toVec3(origin);
    const y = toVec3(yDir);
    const start = o.clone().add(y.clone().multiplyScalar(extents.minV));
    const end = o.clone().add(y.clone().multiplyScalar(extents.maxV));
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [origin, yDir, extents]);

  // Origin marker position
  const originPos = useMemo(() => toVec3(origin), [origin]);

  return (
    <group>
      {/* Minor grid lines */}
      {gridLines.map((line, i) => (
        <Line
          key={i}
          points={line.points}
          color={isDark ? "#404040" : "#888888"}
          lineWidth={line.width}
          transparent
          opacity={0.3}
          depthWrite={false}
        />
      ))}

      {/* X axis (red) */}
      <Line
        points={xAxisPoints}
        color="#ef4444"
        lineWidth={2}
        transparent
        opacity={0.8}
        depthWrite={false}
      />

      {/* Y axis (green) */}
      <Line
        points={yAxisPoints}
        color="#22c55e"
        lineWidth={2}
        transparent
        opacity={0.8}
        depthWrite={false}
      />

      {/* Origin marker */}
      <mesh position={originPos}>
        <sphereGeometry args={[1, 16, 16]} />
        <meshBasicMaterial color="#ffffff" />
      </mesh>
    </group>
  );
}

interface SketchGeometry3DProps {
  segments: SketchSegment2D[];
  selectedSegments: number[];
  constraints: SketchConstraint[];
  pendingPoints: Vec2[];
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
}

/** Render sketch segments as 3D lines */
function SketchGeometry3D({
  segments,
  selectedSegments,
  constraints,
  pendingPoints,
  origin,
  xDir,
  yDir,
}: SketchGeometry3DProps) {
  const { isDark } = useTheme();

  // Convert 2D segment endpoints to 3D
  const segmentLines = useMemo(() => {
    return segments.map((seg, i) => {
      const isSelected = selectedSegments.includes(i);
      const start = sketchToWorld(seg.start, origin, xDir, yDir);
      const end = sketchToWorld(seg.end, origin, xDir, yDir);

      if (seg.type === "Line") {
        return {
          type: "line" as const,
          points: [
            [start.x, start.y, start.z],
            [end.x, end.y, end.z],
          ] as [number, number, number][],
          color: isSelected ? "#f59e0b" : "#3b82f6",
          width: isSelected ? 3 : 2,
          start,
          end,
        };
      } else {
        // Arc: subdivide into line segments for rendering
        const center = sketchToWorld(seg.center, origin, xDir, yDir);
        const radius = start.distanceTo(center);
        const startAngle = Math.atan2(
          seg.start.y - seg.center.y,
          seg.start.x - seg.center.x,
        );
        const endAngle = Math.atan2(
          seg.end.y - seg.center.y,
          seg.end.x - seg.center.x,
        );

        const points: [number, number, number][] = [];
        const steps = 16;
        let angleDiff = endAngle - startAngle;
        if (seg.ccw && angleDiff < 0) angleDiff += 2 * Math.PI;
        if (!seg.ccw && angleDiff > 0) angleDiff -= 2 * Math.PI;

        for (let j = 0; j <= steps; j++) {
          const t = j / steps;
          const angle = startAngle + angleDiff * t;
          const pt2d: Vec2 = {
            x: seg.center.x + radius * Math.cos(angle),
            y: seg.center.y + radius * Math.sin(angle),
          };
          const pt3d = sketchToWorld(pt2d, origin, xDir, yDir);
          points.push([pt3d.x, pt3d.y, pt3d.z]);
        }

        return {
          type: "arc" as const,
          points,
          color: isSelected ? "#f59e0b" : "#3b82f6",
          width: isSelected ? 3 : 2,
          start,
          end,
        };
      }
    });
  }, [segments, selectedSegments, origin, xDir, yDir]);

  // Collect unique vertices for rendering
  const vertices = useMemo(() => {
    const pts: THREE.Vector3[] = [];
    const seen = new Set<string>();
    for (const seg of segments) {
      const start = sketchToWorld(seg.start, origin, xDir, yDir);
      const end = sketchToWorld(seg.end, origin, xDir, yDir);
      const startKey = `${start.x.toFixed(2)},${start.y.toFixed(
        2,
      )},${start.z.toFixed(2)}`;
      const endKey = `${end.x.toFixed(2)},${end.y.toFixed(2)},${end.z.toFixed(
        2,
      )}`;
      if (!seen.has(startKey)) {
        seen.add(startKey);
        pts.push(start);
      }
      if (!seen.has(endKey)) {
        seen.add(endKey);
        pts.push(end);
      }
    }
    return pts;
  }, [segments, origin, xDir, yDir]);

  // Pending points (orange)
  const pendingPoints3D = useMemo(() => {
    return pendingPoints.map((pt) => sketchToWorld(pt, origin, xDir, yDir));
  }, [pendingPoints, origin, xDir, yDir]);

  // Constraint labels
  const constraintLabels = useMemo(() => {
    const labels: { position: THREE.Vector3; text: string; color: string }[] =
      [];

    for (const constraint of constraints) {
      if (constraint.type === "Horizontal") {
        const seg = segments[constraint.line];
        if (seg?.type === "Line") {
          const mid: Vec2 = {
            x: (seg.start.x + seg.end.x) / 2,
            y: (seg.start.y + seg.end.y) / 2 + 3,
          };
          labels.push({
            position: sketchToWorld(mid, origin, xDir, yDir),
            text: "H",
            color: "#22c55e",
          });
        }
      } else if (constraint.type === "Vertical") {
        const seg = segments[constraint.line];
        if (seg?.type === "Line") {
          const mid: Vec2 = {
            x: (seg.start.x + seg.end.x) / 2 + 3,
            y: (seg.start.y + seg.end.y) / 2,
          };
          labels.push({
            position: sketchToWorld(mid, origin, xDir, yDir),
            text: "V",
            color: "#22c55e",
          });
        }
      } else if (constraint.type === "Length") {
        const seg = segments[constraint.line];
        if (seg?.type === "Line") {
          const mid: Vec2 = {
            x: (seg.start.x + seg.end.x) / 2,
            y: (seg.start.y + seg.end.y) / 2 - 3,
          };
          labels.push({
            position: sketchToWorld(mid, origin, xDir, yDir),
            text: `${constraint.length}`,
            color: "#a855f7",
          });
        }
      } else if (constraint.type === "Parallel") {
        const segA = segments[constraint.lineA];
        const segB = segments[constraint.lineB];
        if (segA?.type === "Line" && segB?.type === "Line") {
          const midA: Vec2 = {
            x: (segA.start.x + segA.end.x) / 2,
            y: (segA.start.y + segA.end.y) / 2 + 3,
          };
          const midB: Vec2 = {
            x: (segB.start.x + segB.end.x) / 2,
            y: (segB.start.y + segB.end.y) / 2 + 3,
          };
          labels.push({
            position: sketchToWorld(midA, origin, xDir, yDir),
            text: "//",
            color: "#06b6d4",
          });
          labels.push({
            position: sketchToWorld(midB, origin, xDir, yDir),
            text: "//",
            color: "#06b6d4",
          });
        }
      } else if (constraint.type === "Perpendicular") {
        const segA = segments[constraint.lineA];
        if (segA?.type === "Line") {
          const mid: Vec2 = {
            x: (segA.start.x + segA.end.x) / 2,
            y: (segA.start.y + segA.end.y) / 2 + 3,
          };
          labels.push({
            position: sketchToWorld(mid, origin, xDir, yDir),
            text: "\u22a5",
            color: "#f43f5e",
          });
        }
      } else if (constraint.type === "EqualLength") {
        const segA = segments[constraint.lineA];
        const segB = segments[constraint.lineB];
        if (segA?.type === "Line" && segB?.type === "Line") {
          const midA: Vec2 = {
            x: (segA.start.x + segA.end.x) / 2,
            y: (segA.start.y + segA.end.y) / 2 + 3,
          };
          const midB: Vec2 = {
            x: (segB.start.x + segB.end.x) / 2,
            y: (segB.start.y + segB.end.y) / 2 + 3,
          };
          labels.push({
            position: sketchToWorld(midA, origin, xDir, yDir),
            text: "=",
            color: "#eab308",
          });
          labels.push({
            position: sketchToWorld(midB, origin, xDir, yDir),
            text: "=",
            color: "#eab308",
          });
        }
      }
    }

    return labels;
  }, [constraints, segments, origin, xDir, yDir]);

  return (
    <group>
      {/* Segment lines */}
      {segmentLines.map((line, i) => (
        <Line
          key={i}
          points={line.points}
          color={line.color}
          lineWidth={line.width}
          depthWrite={false}
        />
      ))}

      {/* Vertices — fixed 4px screen radius */}
      {vertices.map((v, i) => (
        <ScreenScaledGroup key={i} position={v}>
          <mesh>
            <sphereGeometry args={[4, 12, 12]} />
            <meshBasicMaterial color={isDark ? "#00d4ff" : "#0891b2"} />
          </mesh>
        </ScreenScaledGroup>
      ))}

      {/* Pending points (during shape creation) — fixed 5px screen radius */}
      {pendingPoints3D.map((pt, i) => (
        <ScreenScaledGroup key={`pending-${i}`} position={pt}>
          <mesh>
            <sphereGeometry args={[5, 12, 12]} />
            <meshBasicMaterial color="#f59e0b" />
          </mesh>
        </ScreenScaledGroup>
      ))}

      {/* Constraint labels */}
      {constraintLabels.map((label, i) => (
        <Html key={i} position={label.position} center>
          <div
            className="pointer-events-none select-none whitespace-nowrap text-xs font-bold"
            style={{ color: label.color }}
          >
            {label.text}
          </div>
        </Html>
      ))}
    </group>
  );
}

interface SketchCursor3DProps {
  cursorWorldPos: Vec3 | null;
  cursorSketchPos: Vec2 | null;
  snapTarget: Vec2 | null;
  gridSnap: boolean;
  previewLine: { start: Vec2; end: Vec2 } | null;
  previewRect: { p1: Vec2; p2: Vec2 } | null;
  previewCircle: { center: Vec2; radius: number } | null;
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
}

/** Cursor, crosshair, and preview shapes */
function SketchCursor3D({
  cursorWorldPos,
  cursorSketchPos,
  snapTarget,
  gridSnap,
  previewLine,
  previewRect,
  previewCircle,
  origin,
  xDir,
  yDir,
}: SketchCursor3DProps) {
  const cursorPos = useMemo(
    () => (cursorWorldPos ? toVec3(cursorWorldPos) : null),
    [cursorWorldPos],
  );
  // Crosshair points are expressed in local frame (centered at origin) and
  // wrapped in a ScreenScaledGroup so they render at a fixed pixel size at
  // any zoom. CROSSHAIR_PX is the half-length in pixels.
  const CROSSHAIR_PX = 14;

  const xCross = useMemo(() => {
    const x = toVec3(xDir);
    return [
      [-x.x * CROSSHAIR_PX, -x.y * CROSSHAIR_PX, -x.z * CROSSHAIR_PX],
      [x.x * CROSSHAIR_PX, x.y * CROSSHAIR_PX, x.z * CROSSHAIR_PX],
    ] as [number, number, number][];
  }, [xDir]);

  const yCross = useMemo(() => {
    const y = toVec3(yDir);
    return [
      [-y.x * CROSSHAIR_PX, -y.y * CROSSHAIR_PX, -y.z * CROSSHAIR_PX],
      [y.x * CROSSHAIR_PX, y.y * CROSSHAIR_PX, y.z * CROSSHAIR_PX],
    ] as [number, number, number][];
  }, [yDir]);

  // Preview line
  const previewLinePoints = useMemo(() => {
    if (!previewLine) return null;
    const start = sketchToWorld(previewLine.start, origin, xDir, yDir);
    const end = sketchToWorld(previewLine.end, origin, xDir, yDir);
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [previewLine, origin, xDir, yDir]);

  // Preview rectangle
  const previewRectPoints = useMemo(() => {
    if (!previewRect) return null;
    const { p1, p2 } = previewRect;
    const minX = Math.min(p1.x, p2.x);
    const maxX = Math.max(p1.x, p2.x);
    const minY = Math.min(p1.y, p2.y);
    const maxY = Math.max(p1.y, p2.y);

    const corners = [
      { x: minX, y: minY },
      { x: maxX, y: minY },
      { x: maxX, y: maxY },
      { x: minX, y: maxY },
      { x: minX, y: minY },
    ];

    return corners.map((c) => {
      const pt = sketchToWorld(c, origin, xDir, yDir);
      return [pt.x, pt.y, pt.z] as [number, number, number];
    });
  }, [previewRect, origin, xDir, yDir]);

  // Preview circle
  const previewCirclePoints = useMemo(() => {
    if (!previewCircle || previewCircle.radius < 0.1) return null;
    const { center, radius } = previewCircle;
    const steps = 32;
    const points: [number, number, number][] = [];

    for (let i = 0; i <= steps; i++) {
      const angle = (2 * Math.PI * i) / steps;
      const pt2d: Vec2 = {
        x: center.x + radius * Math.cos(angle),
        y: center.y + radius * Math.sin(angle),
      };
      const pt3d = sketchToWorld(pt2d, origin, xDir, yDir);
      points.push([pt3d.x, pt3d.y, pt3d.z]);
    }

    return points;
  }, [previewCircle, origin, xDir, yDir]);

  // Snap indicator position
  const snapPos = useMemo(() => {
    if (!snapTarget) return null;
    return sketchToWorld(snapTarget, origin, xDir, yDir);
  }, [snapTarget, origin, xDir, yDir]);

  if (!cursorPos || !cursorSketchPos || !xCross || !yCross) return null;

  return (
    <group>
      {/* Crosshair - cyan when grid snap active. Pixel-sized so it stays
          legible at any zoom level. */}
      <ScreenScaledGroup position={cursorPos}>
        <Line
          points={xCross}
          color={gridSnap && !snapTarget ? "#06b6d4" : "rgba(255,255,255,0.5)"}
          lineWidth={gridSnap && !snapTarget ? 1.5 : 1}
          depthWrite={false}
        />
        <Line
          points={yCross}
          color={gridSnap && !snapTarget ? "#06b6d4" : "rgba(255,255,255,0.5)"}
          lineWidth={gridSnap && !snapTarget ? 1.5 : 1}
          depthWrite={false}
        />
      </ScreenScaledGroup>

      {/* Preview line (dashed) */}
      {previewLinePoints && (
        <Line
          points={previewLinePoints}
          color="rgba(59, 130, 246, 0.6)"
          lineWidth={2}
          dashed
          dashSize={2}
          gapSize={2}
          depthWrite={false}
        />
      )}

      {/* Preview rectangle (dashed) */}
      {previewRectPoints && (
        <Line
          points={previewRectPoints}
          color="rgba(59, 130, 246, 0.6)"
          lineWidth={2}
          dashed
          dashSize={2}
          gapSize={2}
          depthWrite={false}
        />
      )}

      {/* Preview circle (dashed) */}
      {previewCirclePoints && (
        <Line
          points={previewCirclePoints}
          color="rgba(59, 130, 246, 0.6)"
          lineWidth={2}
          dashed
          dashSize={2}
          gapSize={2}
          depthWrite={false}
        />
      )}

      {/* Point snap indicator (green) — fixed pixel size at any zoom level */}
      {snapPos && (
        <ScreenScaledGroup position={snapPos}>
          <mesh>
            <ringGeometry args={[8, 10, 24]} />
            <meshBasicMaterial color="#22c55e" side={THREE.DoubleSide} />
          </mesh>
          <mesh>
            <sphereGeometry args={[3, 12, 12]} />
            <meshBasicMaterial color="#22c55e" />
          </mesh>
        </ScreenScaledGroup>
      )}

      {/* Coordinate label */}
      <Html position={cursorPos} style={{ pointerEvents: "none" }}>
        <div className="ml-4 -mt-2 whitespace-nowrap rounded bg-black/70 px-1.5 py-0.5 text-xs text-white">
          {cursorSketchPos.x.toFixed(1)}, {cursorSketchPos.y.toFixed(1)}
        </div>
      </Html>
    </group>
  );
}

/** Main 3D sketch plane component */
export function SketchPlane3D() {
  const { raycaster } = useThree();

  // Sketch store state
  const active = useSketchStore((s) => s.active);
  const plane = useSketchStore((s) => s.plane);
  const origin = useSketchStore((s) => s.origin);
  const segments = useSketchStore((s) => s.segments);
  const constraints = useSketchStore((s) => s.constraints);
  const tool = useSketchStore((s) => s.tool);
  const constraintTool = useSketchStore((s) => s.constraintTool);
  const points = useSketchStore((s) => s.points);
  const selectedSegments = useSketchStore((s) => s.selectedSegments);
  const cursorWorldPos = useSketchStore((s) => s.cursorWorldPos);
  const cursorSketchPos = useSketchStore((s) => s.cursorSketchPos);
  const snapTarget = useSketchStore((s) => s.snapTarget);
  const selectedFace = useSketchStore((s) => s.selectedFace);
  const addPoint = useSketchStore((s) => s.addPoint);
  const finishShape = useSketchStore((s) => s.finishShape);
  const toggleSegmentSelection = useSketchStore(
    (s) => s.toggleSegmentSelection,
  );
  const setCursorPos = useSketchStore((s) => s.setCursorPos);

  const gridSnap = useUiStore((s) => s.gridSnap);
  const pointSnap = useUiStore((s) => s.pointSnap);

  const isConstraintMode = constraintTool !== "none";

  // Plane basis (origin + in-plane axes + normal) sourced from the kernel
  // so the web app, the TUI, and the WASM SketchSession all use the same
  // math. We merge in the store's `origin` because axis-aligned planes
  // don't carry their own origin.
  const planeVectors = useMemo(() => {
    const basis = getPlaneBasis(plane);
    return { ...basis, origin };
  }, [plane, origin]);

  // Project the selected face's coplanar vertices into 2D sketch (U/V)
  // coordinates. Used to size the grid to the face and to provide snap
  // targets at the face's corners. Only populated when sketching on a
  // real part face — null on XY/XZ/YZ reference planes.
  const faceProjection = useMemo(() => {
    const verts = selectedFace?.vertices;
    if (!verts || verts.length === 0) return null;
    const projected: Vec2[] = verts.map((v) =>
      worldToSketchFast(
        toVec3(v),
        planeVectors.origin,
        planeVectors.xDir,
        planeVectors.yDir,
      ),
    );
    let minU = Infinity;
    let maxU = -Infinity;
    let minV = Infinity;
    let maxV = -Infinity;
    for (const p of projected) {
      if (p.x < minU) minU = p.x;
      if (p.x > maxU) maxU = p.x;
      if (p.y < minV) minV = p.y;
      if (p.y > maxV) maxV = p.y;
    }
    return { vertices: projected, bounds: { minU, maxU, minV, maxV } };
  }, [selectedFace, planeVectors]);

  // Build the segment list we'll feed to the kernel snap/hit-test helpers.
  // Face corners (when sketching on a real face) are modelled as
  // degenerate line segments so the kernel's vertex-snap pass picks them
  // up — keeps the snap logic in a single place instead of maintaining a
  // parallel vertex list here.
  const snapSegments = useMemo(() => {
    if (!faceProjection || faceProjection.vertices.length === 0) {
      return segments;
    }
    const extra: SketchSegment2D[] = faceProjection.vertices.map((v) => ({
      type: "Line",
      start: v,
      end: v,
    }));
    return [...segments, ...extra];
  }, [segments, faceProjection]);

  // Snap function — forwards to the kernel-backed helper so the rules
  // (vertex priority over grid, configurable tolerances) stay in sync
  // with the TUI and any future WASM-driven sketch clients.
  const snap = useCallback(
    (pt: Vec2): { snapped: Vec2; target: Vec2 | null } => {
      const { snapped, target } = snapPoint(snapSegments, pt, {
        gridEnabled: gridSnap,
        gridSize: GRID_SIZE,
        pointEnabled: pointSnap,
        pointTolerance: POINT_SNAP_TOLERANCE,
      });
      return { snapped, target };
    },
    [pointSnap, gridSnap, snapSegments],
  );

  // Invisible plane mesh for raycasting. The mesh exists only as an event
  // capture target; it's deliberately huge (effectively infinite within the
  // camera's 10 000 mm far plane) so clicks work anywhere on screen — no
  // hard dead zone at the edge of a small square. Frustum culling is
  // disabled for the same reason: an oversized bounding sphere centered on
  // the sketch origin would otherwise drop out of view when the camera
  // pans far away.
  const planeMeshRef = useRef<THREE.Mesh>(null);
  const RAYCAST_PLANE_SIZE = 50000; // mm

  // Create plane geometry oriented to sketch plane
  const planeGeometry = useMemo(() => {
    const geo = new THREE.PlaneGeometry(RAYCAST_PLANE_SIZE, RAYCAST_PLANE_SIZE);

    // Build rotation matrix from normal
    const n = toVec3(planeVectors.normal);
    const up = new THREE.Vector3(0, 0, 1);
    const quaternion = new THREE.Quaternion().setFromUnitVectors(up, n);

    geo.applyQuaternion(quaternion);
    geo.translate(
      planeVectors.origin.x,
      planeVectors.origin.y,
      planeVectors.origin.z,
    );

    return geo;
  }, [planeVectors]);

  // Raycast to plane and update cursor
  const handlePointerMove = useCallback(
    () => {
      if (!planeMeshRef.current) return;

      // Raycast to the plane
      const intersects = raycaster.intersectObject(planeMeshRef.current);
      if (intersects.length === 0) {
        setCursorPos(null, null, null);
        return;
      }

      // `intersects[0].point` is in Three world (Y-up) space, but SketchPlane3D
      // lives inside the Z-up→Y-up rotation group in ViewportContent. The
      // sketch basis vectors are in kernel (Z-up) space, so convert back to
      // that frame before projecting.
      const localPt = planeMeshRef.current.worldToLocal(
        intersects[0]!.point.clone(),
      );
      const sketchPt = worldToSketchFast(
        localPt,
        planeVectors.origin,
        planeVectors.xDir,
        planeVectors.yDir,
      );
      const { snapped, target } = snap(sketchPt);

      // Convert snapped point back to world coords
      const snappedWorld = sketchToWorld(
        snapped,
        planeVectors.origin,
        planeVectors.xDir,
        planeVectors.yDir,
      );

      setCursorPos(
        { x: snappedWorld.x, y: snappedWorld.y, z: snappedWorld.z },
        snapped,
        target,
      );
    },
    [planeVectors, raycaster, snap, setCursorPos],
  );

  // Handle click on plane
  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      if (e.button !== 0) return; // Left click only
      // Ignore the click that follows a camera rotate/pan gesture.
      if (viewportWasDrag()) return;
      e.stopPropagation();

      if (!cursorSketchPos) return;

      if (isConstraintMode) {
        // In constraint mode, select/deselect segments via the kernel
        // hit-test (shared with the TUI and the WASM session).
        const idx = hitTestSegments(segments, cursorSketchPos, 2);
        if (idx !== null) {
          toggleSegmentSelection(idx);
        }
      } else {
        // Normal drawing mode
        addPoint(cursorSketchPos);
      }
    },
    [
      cursorSketchPos,
      isConstraintMode,
      segments,
      addPoint,
      toggleSegmentSelection,
    ],
  );

  // Handle double click to finish shape
  const handleDoubleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation();
      if (tool === "line") {
        finishShape();
      }
    },
    [tool, finishShape],
  );

  // Handle pointer leave
  const handlePointerLeave = useCallback(() => {
    setCursorPos(null, null, null);
  }, [setCursorPos]);

  // Preview shapes based on tool and pending points
  const previewLine = useMemo(() => {
    if (tool === "line" && points.length > 0 && cursorSketchPos) {
      return { start: points[points.length - 1]!, end: cursorSketchPos };
    }
    return null;
  }, [tool, points, cursorSketchPos]);

  const previewRect = useMemo(() => {
    if (tool === "rectangle" && points.length === 1 && cursorSketchPos) {
      return { p1: points[0]!, p2: cursorSketchPos };
    }
    return null;
  }, [tool, points, cursorSketchPos]);

  const previewCircle = useMemo(() => {
    if (tool === "circle" && points.length === 1 && cursorSketchPos) {
      const center = points[0]!;
      const radius = Math.sqrt(
        (cursorSketchPos.x - center.x) ** 2 +
          (cursorSketchPos.y - center.y) ** 2,
      );
      return { center, radius };
    }
    return null;
  }, [tool, points, cursorSketchPos]);

  if (!active) return null;

  return (
    <group>
      {/* Invisible plane for raycasting — oversized + frustumCulled off so
          clicks land anywhere on screen, not just inside a 400 mm square.
          No semi-transparent backdrop: the real 3D scene stays visible
          through the sketch. */}
      <mesh
        ref={planeMeshRef}
        geometry={planeGeometry}
        frustumCulled={false}
        onPointerMove={handlePointerMove}
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
        onPointerLeave={handlePointerLeave}
      >
        <meshBasicMaterial visible={false} side={THREE.DoubleSide} />
      </mesh>

      {/* Grid — sized to the face when sketching on one, otherwise a
          centered ±GRID_EXTENT square on a reference plane. */}
      <SketchGrid3D
        origin={planeVectors.origin}
        xDir={planeVectors.xDir}
        yDir={planeVectors.yDir}
        bounds={faceProjection?.bounds ?? null}
      />

      {/* Sketch geometry (segments, vertices, constraints) */}
      <SketchGeometry3D
        segments={segments}
        selectedSegments={selectedSegments}
        constraints={constraints}
        pendingPoints={points}
        origin={planeVectors.origin}
        xDir={planeVectors.xDir}
        yDir={planeVectors.yDir}
      />

      {/* Cursor and preview */}
      <SketchCursor3D
        cursorWorldPos={cursorWorldPos}
        cursorSketchPos={cursorSketchPos}
        snapTarget={snapTarget}
        gridSnap={gridSnap}
        previewLine={previewLine}
        previewRect={previewRect}
        previewCircle={previewCircle}
        origin={planeVectors.origin}
        xDir={planeVectors.xDir}
        yDir={planeVectors.yDir}
      />
    </group>
  );
}
