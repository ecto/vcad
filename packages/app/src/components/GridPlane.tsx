import { useMemo } from "react";
import { Grid, Line } from "@react-three/drei";
import { useTheme } from "@/hooks/useTheme";

export function GridPlane() {
  const { isDark } = useTheme();

  // Axis lines at origin - RGB convention (X=red, Y=green, Z=blue)
  const xAxisPoints = useMemo(
    () =>
      [
        [-500, 0, 0],
        [500, 0, 0],
      ] as [number, number, number][],
    [],
  );

  const yAxisPoints = useMemo(
    () =>
      [
        [0, 0, 0],
        [0, 500, 0],
      ] as [number, number, number][],
    [],
  );

  const zAxisPoints = useMemo(
    () =>
      [
        [0, 0, -500],
        [0, 0, 500],
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
      {/* X axis - red */}
      <Line
        points={xAxisPoints}
        color={isDark ? "#e06c75" : "#c94f4f"}
        lineWidth={1.5}
        transparent
        opacity={0.7}
        depthWrite={false}
        depthTest={false}
        renderOrder={1}
      />
      {/* Y axis - green */}
      <Line
        points={yAxisPoints}
        color={isDark ? "#98c379" : "#5a9a4a"}
        lineWidth={1.5}
        transparent
        opacity={0.7}
        depthWrite={false}
        depthTest={false}
        renderOrder={1}
      />
      {/* Z axis - blue */}
      <Line
        points={zAxisPoints}
        color={isDark ? "#61afef" : "#4a7dc9"}
        lineWidth={1.5}
        transparent
        opacity={0.7}
        depthWrite={false}
        depthTest={false}
        renderOrder={1}
      />
    </>
  );
}
