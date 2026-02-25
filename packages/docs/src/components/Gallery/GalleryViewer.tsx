"use client";

import { useRef, useState, useEffect, useMemo, Suspense } from "react";
import { Canvas, useThree, useFrame } from "@react-three/fiber";
import { OrbitControls, Environment, ContactShadows } from "@react-three/drei";
import * as THREE from "three";
import type { Document, MaterialDef } from "@vcad/ir";
import type { TriangleMesh } from "@vcad/engine";
import { evaluateDocument } from "@/lib/vcad";
import { useTheme } from "@/components/ThemeProvider";

const BG_DARK = "#09090b";
const BG_LIGHT = "#f4f4f5";

function SceneMesh({ mesh, material }: { mesh: TriangleMesh; material: MaterialDef }) {
  const geometry = useMemo(() => {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.Float32BufferAttribute(mesh.positions, 3));
    geo.setIndex(new THREE.Uint32BufferAttribute(mesh.indices, 1));
    geo.computeVertexNormals();
    return geo;
  }, [mesh]);

  const materialProps = useMemo(() => ({
    color: new THREE.Color(material.color[0], material.color[1], material.color[2]),
    metalness: material.metallic,
    roughness: material.roughness,
  }), [material]);

  return (
    <mesh geometry={geometry} castShadow receiveShadow>
      <meshStandardMaterial {...materialProps} />
    </mesh>
  );
}

function AutoRotateControls() {
  const controlsRef = useRef<any>(null);

  useFrame(() => {
    if (!controlsRef.current) return;
    controlsRef.current.autoRotate = true;
    controlsRef.current.autoRotateSpeed = 1.0;
  });

  return (
    <OrbitControls
      ref={controlsRef}
      makeDefault
      enableDamping
      dampingFactor={0.1}
      enableZoom={false}
      enablePan={false}
    />
  );
}

interface MeshData {
  mesh: TriangleMesh;
  material: MaterialDef;
}

function ViewportContent({ meshData }: { meshData: MeshData[] }) {
  const { theme } = useTheme();
  const isDark = theme === "dark";

  return (
    <>
      <Environment preset="studio" environmentIntensity={0.4} />
      <directionalLight position={[50, 80, 40]} intensity={1.2} />
      <directionalLight position={[-30, 40, -20]} intensity={0.4} />
      <ambientLight intensity={0.2} />
      <ContactShadows
        position={[0, -0.01, 0]}
        opacity={isDark ? 0.4 : 0.3}
        scale={100}
        blur={2}
        far={50}
        resolution={128}
        color={isDark ? "#000000" : "#1a1a1a"}
      />
      {meshData.map((data, idx) => (
        <SceneMesh key={idx} mesh={data.mesh} material={data.material} />
      ))}
      <AutoRotateControls />
    </>
  );
}

export function GalleryViewer({ document }: { document: Document }) {
  const { theme } = useTheme();
  const [meshData, setMeshData] = useState<MeshData[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;

    async function evaluate() {
      try {
        const scene = await evaluateDocument(document);
        if (cancelled) return;

        const data: MeshData[] = scene.parts.map((part, idx) => {
          const materialKey = document.roots[idx]?.material ?? "default";
          const material: MaterialDef = document.materials[materialKey] ?? {
            name: "Default",
            color: [0.8, 0.8, 0.8],
            metallic: 0.5,
            roughness: 0.5,
          };
          return { mesh: part.mesh, material };
        });

        setMeshData(data);
      } catch {
        if (!cancelled) setError(true);
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    evaluate();
    return () => { cancelled = true; };
  }, [document]);

  const isDark = theme === "dark";

  if (error) {
    return (
      <div className="aspect-square bg-card flex items-center justify-center">
        <div className="text-4xl text-text-muted opacity-30">&#x25C7;</div>
      </div>
    );
  }

  return (
    <div className="aspect-square relative overflow-hidden">
      <Canvas
        camera={{ position: [60, 50, 60], fov: 45, near: 0.1, far: 1000 }}
        gl={{
          antialias: true,
          toneMapping: THREE.ACESFilmicToneMapping,
          toneMappingExposure: 1.0,
        }}
        style={{ background: isDark ? BG_DARK : BG_LIGHT }}
      >
        <Suspense fallback={null}>
          {meshData.length > 0 && <ViewportContent meshData={meshData} />}
        </Suspense>
      </Canvas>
      {loading && (
        <div className="absolute inset-0 flex items-center justify-center bg-card">
          <div className="text-text-muted text-xs animate-pulse">Loading...</div>
        </div>
      )}
    </div>
  );
}
