import { useRef, useEffect, useMemo, useState, useCallback } from "react";
import { Canvas, useThree } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useTheme } from "@/hooks/useTheme";
import * as THREE from "three";

function PcbMesh() {
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const doc = useDocumentStore((s) => s.document);
  const meshRef = useRef<THREE.Mesh>(null);

  const pcb = activeBoardNodeId != null ? getNodePcb(doc, activeBoardNodeId) : null;

  // Build simple geometry from outline
  const geometry = useMemo(() => {
    if (!pcb) return null;
    const verts = pcb.outline.vertices;
    if (verts.length < 3) return null;

    const thickness = pcb.outline.thickness;
    const shape = new THREE.Shape();
    shape.moveTo(verts[0]!.x, verts[0]!.y);
    for (let i = 1; i < verts.length; i++) {
      shape.lineTo(verts[i]!.x, verts[i]!.y);
    }
    shape.closePath();

    const extrudeSettings = {
      depth: thickness,
      bevelEnabled: false,
    };

    return new THREE.ExtrudeGeometry(shape, extrudeSettings);
  }, [pcb]);

  // Auto-fit camera
  const { camera } = useThree();
  useEffect(() => {
    if (!geometry || !meshRef.current) return;
    geometry.computeBoundingBox();
    const bb = geometry.boundingBox;
    if (!bb) return;
    const center = new THREE.Vector3();
    bb.getCenter(center);
    const size = new THREE.Vector3();
    bb.getSize(size);
    const maxDim = Math.max(size.x, size.y, size.z);
    const cam = camera as THREE.PerspectiveCamera;
    cam.position.set(center.x, center.y - maxDim * 0.5, center.z + maxDim * 1.5);
    cam.lookAt(center);
    cam.updateProjectionMatrix();
  }, [geometry, camera]);

  if (!geometry) return null;

  return (
    <mesh ref={meshRef} geometry={geometry} rotation={[-Math.PI / 2, 0, 0]}>
      <meshStandardMaterial color="#0d5a2d" roughness={0.8} metalness={0} />
    </mesh>
  );
}

export function Pcb3dPreview() {
  const togglePreview = useElectronicsStore((s) => s.toggleShow3dPreview);
  const { isDark } = useTheme();

  // Draggable state
  const [pos, setPos] = useState({ x: 16, y: 16 });
  const [size] = useState({ w: 320, h: 240 });
  const dragRef = useRef<{ startX: number; startY: number; startPosX: number; startPosY: number } | null>(null);

  const onDragStart = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    dragRef.current = { startX: e.clientX, startY: e.clientY, startPosX: pos.x, startPosY: pos.y };
    const onMove = (ev: PointerEvent) => {
      if (!dragRef.current) return;
      setPos({
        x: dragRef.current.startPosX + (ev.clientX - dragRef.current.startX),
        y: dragRef.current.startPosY + (ev.clientY - dragRef.current.startY),
      });
    };
    const onUp = () => {
      dragRef.current = null;
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }, [pos]);

  const bgColor = isDark ? "#1a1a1a" : "#f5f5f5";
  const borderColor = isDark ? "#333" : "#ddd";

  return (
    <div
      className="absolute z-30 shadow-xl overflow-hidden"
      style={{
        right: pos.x,
        bottom: pos.y,
        width: size.w,
        height: size.h,
        border: `1px solid ${borderColor}`,
        borderRadius: 4,
        backgroundColor: bgColor,
      }}
    >
      {/* Title bar */}
      <div
        className="flex items-center justify-between px-2 h-6 cursor-move select-none"
        style={{ backgroundColor: isDark ? "#222" : "#eee", borderBottom: `1px solid ${borderColor}` }}
        onPointerDown={onDragStart}
      >
        <span className="text-[10px] text-text-muted">3D Preview</span>
        <button
          onClick={togglePreview}
          className="flex items-center justify-center w-4 h-4 text-text-muted hover:text-text"
        >
          <X size={10} />
        </button>
      </div>
      {/* Canvas */}
      <Canvas
        camera={{ position: [50, -30, 80], fov: 50, near: 0.1, far: 10000 }}
        style={{ height: size.h - 24 }}
      >
        <ambientLight intensity={0.6} />
        <directionalLight position={[50, 50, 100]} intensity={0.8} />
        <PcbMesh />
        <OrbitControls makeDefault />
      </Canvas>
    </div>
  );
}
