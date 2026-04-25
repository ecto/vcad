import { useMemo } from "react";
import { Grid, Line } from "@react-three/drei";
import { useTheme } from "@/hooks/useTheme";
import { useDocumentStore, useUiStore } from "@vcad/core";

export function GridPlane() {
  const { isDark } = useTheme();
  const isOrbiting = useUiStore((s) => s.isOrbiting);
  const hasParts = useDocumentStore((s) => s.parts.length > 0);
  const showAxes = isOrbiting || !hasParts;

  // Axis lines at origin - RGB convention (X=red, Y=green, Z=blue)
  // Grid is outside the Z-up rotation group, so we draw in Three.js Y-up space
  // but label as Z-up: Three.js X→X (red), Three.js Z→Y (green), Three.js Y→Z (blue)
  const xAxisPoints = useMemo(
    () =>
      [
        [-500, 0, 0],
        [500, 0, 0],
      ] as [number, number, number][],
    [],
  );

  // Y axis (green) → lies in the ground plane (Three.js Z direction)
  const yAxisPoints = useMemo(
    () =>
      [
        [0, 0, -500],
        [0, 0, 500],
      ] as [number, number, number][],
    [],
  );

  // Z axis (blue) → vertical (Three.js Y direction)
  const zAxisPoints = useMemo(
    () =>
      [
        [0, 0, 0],
        [0, 500, 0],
      ] as [number, number, number][],
    [],
  );

  return (
    <>
      <Grid
        args={[1000, 1000]}
        cellSize={10}
        cellThickness={0.5}
        cellColor={isDark ? "#2a2a2a" : "#555555"}
        sectionSize={100}
        sectionThickness={1}
        sectionColor={isDark ? "#3a3a3a" : "#333333"}
        fadeDistance={500}
        fadeStrength={1}
        infiniteGrid
      />
      {showAxes && (
        <>
          {/* X axis - red */}
          <Line
            points={xAxisPoints}
            color={isDark ? "#e06c75" : "#c94f4f"}
            lineWidth={1.5}
            transparent
            opacity={0.7}
            depthWrite={false}
            renderOrder={-1}
          />
          {/* Y axis - green */}
          <Line
            points={yAxisPoints}
            color={isDark ? "#98c379" : "#5a9a4a"}
            lineWidth={1.5}
            transparent
            opacity={0.7}
            depthWrite={false}
            renderOrder={-1}
          />
          {/* Z axis - blue */}
          <Line
            points={zAxisPoints}
            color={isDark ? "#61afef" : "#4a7dc9"}
            lineWidth={1.5}
            transparent
            opacity={0.7}
            depthWrite={false}
            renderOrder={-1}
          />
        </>
      )}
    </>
  );
}
