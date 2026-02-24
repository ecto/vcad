"use client";

import { useRef, useState, useEffect, useMemo, Suspense } from "react";
import { Canvas, useThree, useFrame } from "@react-three/fiber";
import { OrbitControls, Environment, ContactShadows, GizmoHelper, GizmoViewport } from "@react-three/drei";
import * as THREE from "three";
import type { Document, MaterialDef } from "@vcad/ir";
import type { TriangleMesh } from "@vcad/engine";
import { evaluateDocument, computeMeshStats } from "@/lib/vcad";
import { useTheme } from "@/components/ThemeProvider";
import { Download, ArrowsClockwise, Cube } from "@phosphor-icons/react";
import { cn } from "@/lib/utils";

interface ViewportProps {
  document: Document;
}

interface MeshData {
  mesh: TriangleMesh;
  material: MaterialDef;
  stats: ReturnType<typeof computeMeshStats>;
}

// Background colors matching app
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

function CameraController() {
  const { camera } = useThree();
  const controlsRef = useRef<any>(null);

  // Smooth auto-rotation when idle
  const [isInteracting, setIsInteracting] = useState(false);
  const lastInteraction = useRef(Date.now());

  useFrame(() => {
    if (!controlsRef.current) return;

    // Auto-rotate after 3 seconds of inactivity
    if (!isInteracting && Date.now() - lastInteraction.current > 3000) {
      controlsRef.current.autoRotate = true;
      controlsRef.current.autoRotateSpeed = 0.5;
    } else {
      controlsRef.current.autoRotate = false;
    }
  });

  return (
    <OrbitControls
      ref={controlsRef}
      makeDefault
      enableDamping
      dampingFactor={0.1}
      onStart={() => {
        setIsInteracting(true);
        lastInteraction.current = Date.now();
      }}
      onEnd={() => {
        setIsInteracting(false);
        lastInteraction.current = Date.now();
      }}
    />
  );
}

function ViewportContent({ meshData }: { meshData: MeshData[] }) {
  const { theme } = useTheme();
  const isDark = theme === "dark";

  return (
    <>
      {/* Environment lighting */}
      <Environment preset="studio" environmentIntensity={0.4} />

      {/* Lights */}
      <directionalLight
        position={[50, 80, 40]}
        intensity={1.2}
        castShadow
        shadow-mapSize-width={1024}
        shadow-mapSize-height={1024}
      />
      <directionalLight position={[-30, 40, -20]} intensity={0.4} />
      <ambientLight intensity={0.2} />

      {/* Contact shadows */}
      <ContactShadows
        position={[0, -0.01, 0]}
        opacity={isDark ? 0.4 : 0.3}
        scale={100}
        blur={2}
        far={50}
        resolution={256}
        color={isDark ? "#000000" : "#1a1a1a"}
      />

      {/* Meshes */}
      {meshData.map((data, idx) => (
        <SceneMesh key={idx} mesh={data.mesh} material={data.material} />
      ))}

      {/* Controls */}
      <CameraController />

      {/* Orientation gizmo */}
      <GizmoHelper alignment="bottom-right" margin={[60, 60]}>
        <GizmoViewport
          axisColors={["#e06c75", "#98c379", "#61afef"]}
          labelColor="#888888"
        />
      </GizmoHelper>
    </>
  );
}

export function Viewport({ document }: ViewportProps) {
  const { theme } = useTheme();
  const [meshData, setMeshData] = useState<MeshData[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Evaluate document when it changes
  useEffect(() => {
    let cancelled = false;

    async function evaluate() {
      setLoading(true);
      setError(null);

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

          return {
            mesh: part.mesh,
            material,
            stats: computeMeshStats(part.mesh),
          };
        });

        setMeshData(data);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to evaluate");
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    }

    evaluate();

    return () => {
      cancelled = true;
    };
  }, [document]);

  // Aggregate stats
  const stats = useMemo(() => {
    if (meshData.length === 0) return null;

    const total = meshData.reduce(
      (acc, d) => ({
        triangles: acc.triangles + d.stats.triangleCount,
        volume: acc.volume + d.stats.volume,
      }),
      { triangles: 0, volume: 0 }
    );

    return total;
  }, [meshData]);

  const isDark = theme === "dark";

  return (
    <div className="relative h-full min-h-[300px] rounded-lg border border-border overflow-hidden">
      {/* Canvas */}
      <Canvas
        camera={{ position: [50, 50, 50], fov: 50, near: 0.1, far: 1000 }}
        shadows
        gl={{
          antialias: true,
          toneMapping: THREE.ACESFilmicToneMapping,
          toneMappingExposure: 1.0,
        }}
        style={{ background: isDark ? BG_DARK : BG_LIGHT }}
      >
        <Suspense fallback={null}>
          <ViewportContent meshData={meshData} />
        </Suspense>
      </Canvas>

      {/* Loading overlay */}
      {loading && (
        <div className="absolute inset-0 flex items-center justify-center bg-surface/80 backdrop-blur-sm">
          <div className="flex items-center gap-2 text-text-muted">
            <ArrowsClockwise size={20} className="animate-spin" />
            <span>Evaluating...</span>
          </div>
        </div>
      )}

      {/* Error overlay */}
      {error && (
        <div className="absolute inset-0 flex items-center justify-center bg-surface/80 backdrop-blur-sm">
          <div className="text-red-500 text-sm px-4 text-center">
            {error}
          </div>
        </div>
      )}

      {/* Stats bar */}
      {stats && !loading && (
        <div className="absolute bottom-0 left-0 right-0 px-3 py-2 bg-bg/90 backdrop-blur-sm border-t border-border">
          <div className="flex items-center justify-between text-xs text-text-muted">
            <div className="flex items-center gap-4">
              <span>
                <Cube size={12} className="inline mr-1" />
                {stats.triangles.toLocaleString()} triangles
              </span>
              <span>
                {stats.volume.toFixed(0)} mm³
              </span>
            </div>
            <div className="flex items-center gap-2">
              <button
                className={cn(
                  "px-2 py-1 rounded text-xs transition-colors",
                  "hover:bg-hover hover:text-text"
                )}
              >
                <Download size={12} className="inline mr-1" />
                STL
              </button>
              <button
                className={cn(
                  "px-2 py-1 rounded text-xs transition-colors",
                  "hover:bg-hover hover:text-text"
                )}
              >
                GLB
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
