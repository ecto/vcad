import { useEffect, useRef, useMemo } from "react";
import * as THREE from "three";
import type { TriangleMesh } from "@vcad/engine";

const zebraVertexShader = /* glsl */ `
  varying vec3 vWorldPosition;
  void main() {
    vec4 worldPos = modelMatrix * vec4(position, 1.0);
    vWorldPosition = worldPos.xyz;
    gl_Position = projectionMatrix * viewMatrix * worldPos;
  }
`;

const zebraFragmentShader = /* glsl */ `
  varying vec3 vWorldPosition;

  void main() {
    // Diagonal stripes in world space
    float stripe = sin((vWorldPosition.x + vWorldPosition.y + vWorldPosition.z) * 8.0);
    float pattern = step(0.0, stripe);

    // Yellow and black zebra pattern
    vec3 yellow = vec3(1.0, 0.85, 0.0);
    vec3 black = vec3(0.1, 0.1, 0.1);
    vec3 color = mix(black, yellow, pattern);

    gl_FragColor = vec4(color, 1.0);
  }
`;

interface ClashMeshProps {
  mesh: TriangleMesh;
}

export function ClashMesh({ mesh }: ClashMeshProps) {
  const geoRef = useRef<THREE.BufferGeometry>(null);

  const shaderMaterial = useMemo(() => {
    return new THREE.ShaderMaterial({
      vertexShader: zebraVertexShader,
      fragmentShader: zebraFragmentShader,
      side: THREE.DoubleSide,
    });
  }, []);

  useEffect(() => {
    const geo = geoRef.current;
    if (!geo) return;

    geo.setAttribute(
      "position",
      new THREE.BufferAttribute(mesh.positions, 3),
    );
    geo.setIndex(new THREE.BufferAttribute(mesh.indices, 1));

    if (mesh.normals && mesh.normals.length === mesh.positions.length) {
      geo.setAttribute("normal", new THREE.BufferAttribute(mesh.normals, 3));
    } else {
      geo.computeVertexNormals();
    }

    return () => {
      geo.dispose();
    };
  }, [mesh]);

  return (
    <mesh material={shaderMaterial} renderOrder={1}>
      <bufferGeometry ref={geoRef} />
    </mesh>
  );
}
