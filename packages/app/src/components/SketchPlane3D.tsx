import { useRef, useMemo, useCallback } from "react";
import * as THREE from "three";
import { useThree, ThreeEvent } from "@react-three/fiber";
import { Line, Html } from "@react-three/drei";
import {
  useSketchStore,
  useUiStore,
  getSketchPlaneDirections,
} from "@vcad/core";
import type { SketchPlane } from "@vcad/core";
import type { Vec2, Vec3, SketchSegment2D, SketchConstraint } from "@vcad/ir";
import { useTheme } from "@/hooks/useTheme";

const GRID_SIZE = 10; // mm
const GRID_EXTENT = 200; // mm from origin
const POINT_SNAP_TOLERANCE = 5; // mm

/** Convert a Vec3 to Three.js Vector3 */
function toVec3(v: Vec3): THREE.Vector3 {
  return new THREE.Vector3(v.x, v.y, v.z);
}

/** Convert 3D world point to 2D sketch coordinates */
function worldToSketch(
  worldPt: THREE.Vector3,
  origin: Vec3,
  xDir: Vec3,
  yDir: Vec3,
): Vec2 {
  const rel = worldPt.clone().sub(toVec3(origin));
  return {
    x: rel.dot(toVec3(xDir)),
    y: rel.dot(toVec3(yDir)),
  };
}

/** Convert 2D sketch coordinates to 3D world point */
function sketchToWorld(
  pt: Vec2,
  origin: Vec3,
  xDir: Vec3,
  yDir: Vec3,
): THREE.Vector3 {
  const o = toVec3(origin);
  const x = toVec3(xDir).multiplyScalar(pt.x);
  const y = toVec3(yDir).multiplyScalar(pt.y);
  return o.add(x).add(y);
}

/** Get plane directions from SketchPlane type */
function getPlaneVectors(
  plane: SketchPlane,
  origin: Vec3,
): { origin: Vec3; xDir: Vec3; yDir: Vec3; normal: Vec3 } {
  const dirs = getSketchPlaneDirections(plane);
  return {
    origin,
    xDir: dirs.x_dir,
    yDir: dirs.y_dir,
    normal: dirs.normal,
  };
}

/** Find the closest segment to a point for selection */
function findClosestSegment(
  pt: Vec2,
  segments: SketchSegment2D[],
): { index: number; distance: number } | null {
  if (segments.length === 0) return null;

  let closest = { index: 0, distance: Infinity };

  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i]!;
    if (seg.type === "Line") {
      const d = pointToSegmentDistance(pt, seg.start, seg.end);
      if (d < closest.distance) {
        closest = { index: i, distance: d };
      }
    } else {
      // Arc: distance to arc path
      const d = Math.sqrt(
        (pt.x - seg.center.x) ** 2 + (pt.y - seg.center.y) ** 2,
      );
      const radius = Math.sqrt(
        (seg.start.x - seg.center.x) ** 2 + (seg.start.y - seg.center.y) ** 2,
      );
      const arcDist = Math.abs(d - radius);
      if (arcDist < closest.distance) {
        closest = { index: i, distance: arcDist };
      }
    }
  }

  return closest;
}

function pointToSegmentDistance(pt: Vec2, a: Vec2, b: Vec2): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  if (len2 < 0.0001) {
    return Math.sqrt((pt.x - a.x) ** 2 + (pt.y - a.y) ** 2);
  }
  let t = ((pt.x - a.x) * dx + (pt.y - a.y) * dy) / len2;
  t = Math.max(0, Math.min(1, t));
  const projX = a.x + t * dx;
  const projY = a.y + t * dy;
  return Math.sqrt((pt.x - projX) ** 2 + (pt.y - projY) ** 2);
}

interface SketchGrid3DProps {
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
}

/** Grid rendered on the sketch plane */
function SketchGrid3D({ origin, xDir, yDir }: SketchGrid3DProps) {
  const { isDark } = useTheme();

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

    // Minor grid lines
    for (let i = -GRID_EXTENT; i <= GRID_EXTENT; i += GRID_SIZE) {
      if (i === 0) continue; // Skip origin lines, we'll draw axes separately

      // Lines parallel to X axis
      const startX = o
        .clone()
        .add(x.clone().multiplyScalar(-GRID_EXTENT))
        .add(y.clone().multiplyScalar(i));
      const endX = o
        .clone()
        .add(x.clone().multiplyScalar(GRID_EXTENT))
        .add(y.clone().multiplyScalar(i));
      lines.push({
        points: [
          [startX.x, startX.y, startX.z],
          [endX.x, endX.y, endX.z],
        ],
        color: isDark ? "rgba(255,255,255,0.1)" : "rgba(0,0,0,0.1)",
        width: 1,
      });

      // Lines parallel to Y axis
      const startY = o
        .clone()
        .add(y.clone().multiplyScalar(-GRID_EXTENT))
        .add(x.clone().multiplyScalar(i));
      const endY = o
        .clone()
        .add(y.clone().multiplyScalar(GRID_EXTENT))
        .add(x.clone().multiplyScalar(i));
      lines.push({
        points: [
          [startY.x, startY.y, startY.z],
          [endY.x, endY.y, endY.z],
        ],
        color: isDark ? "rgba(255,255,255,0.1)" : "rgba(0,0,0,0.1)",
        width: 1,
      });
    }

    return lines;
  }, [origin, xDir, yDir, isDark]);

  // Axis lines
  const xAxisPoints = useMemo(() => {
    const o = toVec3(origin);
    const x = toVec3(xDir);
    const start = o.clone().add(x.clone().multiplyScalar(-GRID_EXTENT));
    const end = o.clone().add(x.clone().multiplyScalar(GRID_EXTENT));
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [origin, xDir]);

  const yAxisPoints = useMemo(() => {
    const o = toVec3(origin);
    const y = toVec3(yDir);
    const start = o.clone().add(y.clone().multiplyScalar(-GRID_EXTENT));
    const end = o.clone().add(y.clone().multiplyScalar(GRID_EXTENT));
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [origin, yDir]);

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

      {/* Vertices */}
      {vertices.map((v, i) => (
        <mesh key={i} position={v}>
          <sphereGeometry args={[0.8, 12, 12]} />
          <meshBasicMaterial color={isDark ? "#00d4ff" : "#0891b2"} />
        </mesh>
      ))}

      {/* Pending points (during shape creation) */}
      {pendingPoints3D.map((pt, i) => (
        <mesh key={`pending-${i}`} position={pt}>
          <sphereGeometry args={[1.2, 12, 12]} />
          <meshBasicMaterial color="#f59e0b" />
        </mesh>
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
  const crosshairSize = 3;

  // Crosshair lines
  const xCross = useMemo(() => {
    if (!cursorPos) return null;
    const x = toVec3(xDir);
    const start = cursorPos
      .clone()
      .sub(x.clone().multiplyScalar(crosshairSize));
    const end = cursorPos.clone().add(x.clone().multiplyScalar(crosshairSize));
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [cursorPos, xDir]);

  const yCross = useMemo(() => {
    if (!cursorPos) return null;
    const y = toVec3(yDir);
    const start = cursorPos
      .clone()
      .sub(y.clone().multiplyScalar(crosshairSize));
    const end = cursorPos.clone().add(y.clone().multiplyScalar(crosshairSize));
    return [
      [start.x, start.y, start.z],
      [end.x, end.y, end.z],
    ] as [number, number, number][];
  }, [cursorPos, yDir]);

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
      {/* Crosshair - cyan when grid snap active */}
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

      {/* Point snap indicator (green) */}
      {snapPos && (
        <>
          <mesh position={snapPos}>
            <ringGeometry args={[2, 2.5, 16]} />
            <meshBasicMaterial color="#22c55e" side={THREE.DoubleSide} />
          </mesh>
          <mesh position={snapPos}>
            <sphereGeometry args={[0.8, 12, 12]} />
            <meshBasicMaterial color="#22c55e" />
          </mesh>
        </>
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
  const { isDark } = useTheme();
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
  const addPoint = useSketchStore((s) => s.addPoint);
  const finishShape = useSketchStore((s) => s.finishShape);
  const toggleSegmentSelection = useSketchStore(
    (s) => s.toggleSegmentSelection,
  );
  const setCursorPos = useSketchStore((s) => s.setCursorPos);

  const gridSnap = useUiStore((s) => s.gridSnap);
  const pointSnap = useUiStore((s) => s.pointSnap);

  const isConstraintMode = constraintTool !== "none";

  // Get plane vectors
  const planeVectors = useMemo(
    () => getPlaneVectors(plane, origin),
    [plane, origin],
  );

  // Collect all unique vertices from segments for point snapping
  const vertices = useMemo(() => {
    const pts: Vec2[] = [];
    for (const seg of segments) {
      pts.push(seg.start, seg.end);
    }
    // Dedupe by proximity
    const unique: Vec2[] = [];
    for (const pt of pts) {
      const exists = unique.some(
        (u) => Math.abs(u.x - pt.x) < 0.1 && Math.abs(u.y - pt.y) < 0.1,
      );
      if (!exists) {
        unique.push(pt);
      }
    }
    return unique;
  }, [segments]);

  // Snap function
  const snap = useCallback(
    (pt: Vec2): { snapped: Vec2; target: Vec2 | null } => {
      // Priority 1: Point snap
      if (pointSnap && vertices.length > 0) {
        for (const v of vertices) {
          const dist = Math.hypot(pt.x - v.x, pt.y - v.y);
          if (dist < POINT_SNAP_TOLERANCE) {
            return { snapped: v, target: v };
          }
        }
      }

      // Priority 2: Grid snap
      if (gridSnap) {
        return {
          snapped: {
            x: Math.round(pt.x / GRID_SIZE) * GRID_SIZE,
            y: Math.round(pt.y / GRID_SIZE) * GRID_SIZE,
          },
          target: null,
        };
      }

      return { snapped: pt, target: null };
    },
    [pointSnap, gridSnap, vertices],
  );

  // Invisible plane mesh for raycasting
  const planeMeshRef = useRef<THREE.Mesh>(null);

  // Create plane geometry oriented to sketch plane
  const planeGeometry = useMemo(() => {
    const geo = new THREE.PlaneGeometry(GRID_EXTENT * 2, GRID_EXTENT * 2);

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

      const worldPt = intersects[0]!.point;
      const sketchPt = worldToSketch(
        worldPt,
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
      e.stopPropagation();

      if (!cursorSketchPos) return;

      if (isConstraintMode) {
        // In constraint mode, select/deselect segments
        const closest = findClosestSegment(cursorSketchPos, segments);
        if (closest && closest.distance < 2) {
          toggleSegmentSelection(closest.index);
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
      {/* Invisible plane for raycasting */}
      <mesh
        ref={planeMeshRef}
        geometry={planeGeometry}
        onPointerMove={handlePointerMove}
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
        onPointerLeave={handlePointerLeave}
      >
        <meshBasicMaterial visible={false} side={THREE.DoubleSide} />
      </mesh>

      {/* Semi-transparent backdrop */}
      <mesh geometry={planeGeometry}>
        <meshBasicMaterial
          color={isDark ? "#000000" : "#888888"}
          transparent
          opacity={0.3}
          side={THREE.DoubleSide}
          depthWrite={false}
          polygonOffset
          polygonOffsetFactor={-1}
          polygonOffsetUnits={-1}
        />
      </mesh>

      {/* Grid */}
      <SketchGrid3D
        origin={planeVectors.origin}
        xDir={planeVectors.xDir}
        yDir={planeVectors.yDir}
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
