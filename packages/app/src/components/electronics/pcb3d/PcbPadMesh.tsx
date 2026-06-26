/**
 * InstancedMesh pools for PCB pads, one per pad shape type.
 *
 * Pads are grouped across all footprints for efficient batched rendering.
 * Circle pads use cylinder geometry, Rect/Oval/RoundRect use box geometry.
 * Drill holes rendered as dark discs on top.
 */

import { useRef, useMemo, useEffect } from "react";
import * as THREE from "three";
import type { Pcb, Pad, PcbLayer } from "@vcad/ir";
import { layerZ, getLayerColor, isLayerVisible, padDimensions } from "./pcb-geometry";
import type { LayerConfig } from "@/stores/electronics-store";

interface Props {
  pcb: Pcb;
  layers: LayerConfig[];
  activeNet: string | null;
  hoveredNet: string | null;
  explosion: number;
}

interface FlatPad {
  pad: Pad;
  worldX: number;
  worldY: number;
  layer: PcbLayer;
  net: string;
  rotation: number;
}

const ACCENT_COLOR = new THREE.Color("#3b82f6");
const DRILL_COLOR = new THREE.Color("#111111");
const TEMP_MATRIX = new THREE.Matrix4();
const TEMP_COLOR = new THREE.Color();
const TEMP_QUAT = new THREE.Quaternion();
const TEMP_POS = new THREE.Vector3();
const TEMP_SCALE = new THREE.Vector3();

function flattenPads(pcb: Pcb): FlatPad[] {
  const result: FlatPad[] = [];
  for (const fp of pcb.footprints) {
    const fpRot = (fp.rotation ?? 0) * Math.PI / 180;
    for (const pad of fp.pads) {
      // Rotate pad position by footprint rotation
      const cos = Math.cos(fpRot);
      const sin = Math.sin(fpRot);
      const rx = pad.position.x * cos - pad.position.y * sin;
      const ry = pad.position.x * sin + pad.position.y * cos;

      const primaryLayer = pad.layers[0] ?? "FCu";
      result.push({
        pad,
        worldX: fp.position.x + rx,
        worldY: fp.position.y + ry,
        layer: primaryLayer,
        net: pad.net ?? "",
        rotation: fpRot + (pad.rotation ?? 0) * Math.PI / 180,
      });
    }
  }
  return result;
}

export function PcbPadMesh({ pcb, layers, activeNet, hoveredNet, explosion }: Props) {
  const smdMeshRef = useRef<THREE.InstancedMesh>(null);
  const thtMeshRef = useRef<THREE.InstancedMesh>(null);
  const drillMeshRef = useRef<THREE.InstancedMesh>(null);

  const allPads = useMemo(() => flattenPads(pcb), [pcb]);

  const smdPads = useMemo(
    () => allPads.filter((p) => p.pad.padType === "SMD"),
    [allPads],
  );
  const thtPads = useMemo(
    () => allPads.filter((p) => p.pad.padType === "THT" || p.pad.padType === "NPTH"),
    [allPads],
  );

  const thickness = pcb.outline.thickness;

  // Update SMD pad instances
  useEffect(() => {
    const mesh = smdMeshRef.current;
    if (!mesh || smdPads.length === 0) return;

    let count = 0;
    for (const fp of smdPads) {
      if (!isLayerVisible(layers, fp.layer)) continue;
      const [w, h] = padDimensions(fp.pad.shape);
      const z = layerZ(fp.layer, thickness, explosion);

      TEMP_POS.set(fp.worldX, fp.worldY, z);
      TEMP_SCALE.set(w, h, 0.035);
      TEMP_QUAT.setFromAxisAngle(new THREE.Vector3(0, 0, 1), fp.rotation);
      TEMP_MATRIX.compose(TEMP_POS, TEMP_QUAT, TEMP_SCALE);
      mesh.setMatrixAt(count, TEMP_MATRIX);

      const isActive = fp.net === activeNet || fp.net === hoveredNet;
      TEMP_COLOR.set(isActive ? ACCENT_COLOR : new THREE.Color(getLayerColor(layers, fp.layer)));
      mesh.setColorAt(count, TEMP_COLOR);
      count++;
    }

    mesh.count = count;
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  }, [smdPads, layers, activeNet, hoveredNet, thickness, explosion]);

  // Update THT pad instances
  useEffect(() => {
    const mesh = thtMeshRef.current;
    const drillMesh = drillMeshRef.current;
    if (!mesh || thtPads.length === 0) return;

    let count = 0;
    let drillCount = 0;
    for (const fp of thtPads) {
      const [w, h] = padDimensions(fp.pad.shape);
      const z = layerZ("FCu", thickness, explosion);

      // Pad
      TEMP_POS.set(fp.worldX, fp.worldY, z);
      TEMP_SCALE.set(w, h, 0.035);
      TEMP_QUAT.setFromAxisAngle(new THREE.Vector3(0, 0, 1), fp.rotation);
      TEMP_MATRIX.compose(TEMP_POS, TEMP_QUAT, TEMP_SCALE);
      mesh.setMatrixAt(count, TEMP_MATRIX);

      const isActive = fp.net === activeNet || fp.net === hoveredNet;
      TEMP_COLOR.set(isActive ? ACCENT_COLOR : new THREE.Color(getLayerColor(layers, "FCu")));
      mesh.setColorAt(count, TEMP_COLOR);
      count++;

      // Drill hole disc
      if (fp.pad.drill && drillMesh) {
        const drillR = fp.pad.drill.diameter;
        TEMP_POS.set(fp.worldX, fp.worldY, z + 0.01);
        TEMP_SCALE.set(drillR, drillR, 0.01);
        TEMP_QUAT.identity();
        TEMP_MATRIX.compose(TEMP_POS, TEMP_QUAT, TEMP_SCALE);
        drillMesh.setMatrixAt(drillCount, TEMP_MATRIX);
        drillMesh.setColorAt(drillCount, DRILL_COLOR);
        drillCount++;
      }
    }

    mesh.count = count;
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;

    if (drillMesh) {
      drillMesh.count = drillCount;
      drillMesh.instanceMatrix.needsUpdate = true;
      if (drillMesh.instanceColor) drillMesh.instanceColor.needsUpdate = true;
    }
  }, [thtPads, layers, activeNet, hoveredNet, thickness, explosion]);

  return (
    <>
      {/* SMD pads - flat boxes */}
      {smdPads.length > 0 && (
        <instancedMesh
          ref={smdMeshRef}
          args={[undefined, undefined, Math.max(smdPads.length, 1)]}
          frustumCulled={false}
        >
          <boxGeometry args={[1, 1, 1]} />
          <meshStandardMaterial vertexColors roughness={0.32} metalness={0.4} envMapIntensity={1.5} />
        </instancedMesh>
      )}

      {/* THT pads */}
      {thtPads.length > 0 && (
        <instancedMesh
          ref={thtMeshRef}
          args={[undefined, undefined, Math.max(thtPads.length, 1)]}
          frustumCulled={false}
        >
          <boxGeometry args={[1, 1, 1]} />
          <meshStandardMaterial vertexColors roughness={0.32} metalness={0.4} envMapIntensity={1.5} />
        </instancedMesh>
      )}

      {/* Drill holes */}
      {thtPads.length > 0 && (
        <instancedMesh
          ref={drillMeshRef}
          args={[undefined, undefined, Math.max(thtPads.length, 1)]}
          frustumCulled={false}
        >
          <cylinderGeometry args={[0.5, 0.5, 1, 16]} />
          <meshStandardMaterial vertexColors roughness={0.9} metalness={0} />
        </instancedMesh>
      )}
    </>
  );
}
