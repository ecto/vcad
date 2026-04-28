import { useRef, useMemo, useCallback } from "react";
import { Canvas } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import * as THREE from "three";
import { useSlicerStore } from "@/stores/slicer-store";
import { getLayerPreview } from "@/lib/slicer-client";

function LayerMesh() {
  const layerPreview = useSlicerStore((s) => s.currentLayerPreview);
  const meshRef = useRef<THREE.Group>(null);

  // Create geometry from layer preview data
  const geometry = useMemo(() => {
    if (!layerPreview) return null;

    const group = new THREE.Group();

    // Helper to create line from points
    const createLine = (points: [number, number][], color: number, z: number) => {
      if (points.length < 2) return null;
      const positions: number[] = [];
      for (const [x, y] of points) {
        positions.push(x, y, z);
      }
      // Close the loop for perimeters
      if (points.length > 2 && points[0]) {
        positions.push(points[0][0], points[0][1], z);
      }
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
      return new THREE.Line(
        geometry,
        new THREE.LineBasicMaterial({ color, linewidth: 1 })
      );
    };

    // Outer perimeters (orange)
    for (const perimeter of layerPreview.outerPerimeters) {
      const line = createLine(perimeter, 0xff6600, layerPreview.z);
      if (line) group.add(line);
    }

    // Inner perimeters (yellow)
    for (const perimeter of layerPreview.innerPerimeters) {
      const line = createLine(perimeter, 0xffcc00, layerPreview.z);
      if (line) group.add(line);
    }

    // Infill (cyan)
    for (const path of layerPreview.infill) {
      const line = createLine(path, 0x00ccff, layerPreview.z);
      if (line) group.add(line);
    }

    return group;
  }, [layerPreview]);

  if (!geometry) return null;

  return <primitive object={geometry} ref={meshRef} />;
}

function GridHelper() {
  return (
    <gridHelper
      args={[200, 20, 0x444444, 0x333333]}
      rotation={[Math.PI / 2, 0, 0]}
      position={[100, 100, 0]}
    />
  );
}

interface PrintPreviewProps {
  width?: number;
  height?: number;
}

export function PrintPreview({ width = 300, height = 200 }: PrintPreviewProps) {
  const stats = useSlicerStore((s) => s.stats);
  const previewLayerIndex = useSlicerStore((s) => s.previewLayerIndex);
  const setPreviewLayerIndex = useSlicerStore((s) => s.setPreviewLayerIndex);
  const setCurrentLayerPreview = useSlicerStore((s) => s.setCurrentLayerPreview);
  const sliceResult = useSlicerStore((s) => s.sliceResult);

  const layerCount = stats?.layerCount ?? 0;

  const handleLayerChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const index = parseInt(e.target.value);
    setPreviewLayerIndex(index);

    // Round-trip through the slicer worker to fetch this layer's polylines.
    if (sliceResult && index < sliceResult.layerCount) {
      getLayerPreview(sliceResult, index)
        .then(setCurrentLayerPreview)
        .catch((err) => console.error("Failed to get layer preview:", err));
    }
  }, [sliceResult, setPreviewLayerIndex, setCurrentLayerPreview]);

  return (
    <div className="space-y-2">
      {/* 3D Preview */}
      <div
        className="bg-black rounded overflow-hidden"
        style={{ width, height }}
      >
        <Canvas
          camera={{ position: [150, 150, 100], fov: 45 }}
          gl={{ antialias: true }}
        >
          <ambientLight intensity={0.5} />
          <directionalLight position={[10, 10, 10]} intensity={0.8} />
          <GridHelper />
          <LayerMesh />
          <OrbitControls
            enablePan={true}
            enableZoom={true}
            enableRotate={true}
            target={[100, 100, 0]}
          />
        </Canvas>
      </div>

      {/* Layer slider */}
      {layerCount > 0 && (
        <div>
          <div className="flex justify-between text-xs text-text-muted mb-1">
            <span>Layer {previewLayerIndex + 1}</span>
            <span>of {layerCount}</span>
          </div>
          <input
            type="range"
            min="0"
            max={layerCount - 1}
            value={previewLayerIndex}
            onChange={handleLayerChange}
            className="w-full h-2 accent-brand"
          />
        </div>
      )}
    </div>
  );
}
