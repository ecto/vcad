import { useEffect, useRef, useState, useCallback } from "react";
import { TransformControls } from "@react-three/drei";

const isCoarsePointer =
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(pointer: coarse)").matches;
import * as THREE from "three";
import { useUiStore, useDocumentStore } from "@vcad/core";
import type { RefObject } from "react";
import type { OrbitControls as OrbitControlsImpl } from "three-stdlib";

const DEG2RAD = Math.PI / 180;
const RAD2DEG = 180 / Math.PI;

// Kernel Z-up → Three.js Y-up coordinate conversion quaternion
const COORD_QUAT = new THREE.Quaternion().setFromAxisAngle(
  new THREE.Vector3(1, 0, 0),
  -Math.PI / 2,
);
const COORD_QUAT_INV = new THREE.Quaternion().setFromAxisAngle(
  new THREE.Vector3(1, 0, 0),
  Math.PI / 2,
);

/** Kernel (x,y,z) Z-up → display (x,z,-y) Y-up */
function kernelToDisplay(k: { x: number; y: number; z: number }): [number, number, number] {
  return [k.x, k.z, -k.y];
}

/** Display (x,y,z) Y-up → kernel (x,-z,y) Z-up */
function displayToKernel(d: THREE.Vector3): { x: number; y: number; z: number } {
  return { x: d.x, y: -d.z, z: d.y };
}

/** Kernel Euler angles → display quaternion (bakes in coord rotation) */
function kernelEulerToDisplayQuat(
  angles: { x: number; y: number; z: number },
  out: THREE.Quaternion,
): THREE.Quaternion {
  // Kernel euler convention is extrinsic X→Y→Z (matrix Rz·Ry·Rx) = three.js "ZYX".
  const kernelQuat = _tempQuat.setFromEuler(
    _tempEuler.set(angles.x * DEG2RAD, angles.y * DEG2RAD, angles.z * DEG2RAD, "ZYX"),
  );
  return out.copy(COORD_QUAT).multiply(kernelQuat);
}

/** Display quaternion → kernel Euler angles (strips coord rotation) */
function displayQuatToKernelEuler(q: THREE.Quaternion): { x: number; y: number; z: number } {
  _tempQuat.copy(COORD_QUAT_INV).multiply(q);
  _tempEuler.setFromQuaternion(_tempQuat, "ZYX");
  return {
    x: _tempEuler.x * RAD2DEG,
    y: _tempEuler.y * RAD2DEG,
    z: _tempEuler.z * RAD2DEG,
  };
}

// Reusable temp objects to avoid GC pressure during drag
const _tempQuat = new THREE.Quaternion();
const _tempEuler = new THREE.Euler();

export function TransformGizmo({
  orbitControls,
}: {
  orbitControls: RefObject<OrbitControlsImpl | null>;
}) {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const transformMode = useUiStore((s) => s.transformMode);
  const setDraggingGizmo = useUiStore((s) => s.setDraggingGizmo);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const snapIncrement = useUiStore((s) => s.snapIncrement);

  const parts = useDocumentStore((s) => s.parts);
  const document = useDocumentStore((s) => s.document);

  const [proxy, setProxy] = useState<THREE.Object3D | null>(null);
  const proxyCallbackRef = useCallback((obj: THREE.Object3D | null) => {
    setProxy(obj);
  }, []);

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const controlsRef = useRef<any>(null);
  const isDraggingRef = useRef(false);

  // Only show gizmo for single selection
  const singleSelectedId =
    selectedPartIds.size === 1
      ? Array.from(selectedPartIds)[0]!
      : null;

  // Check if selection is a part or an instance
  const selectedPart = singleSelectedId
    ? parts.find((p) => p.id === singleSelectedId)
    : null;
  const selectedInstance = singleSelectedId
    ? document.instances?.find((i) => i.id === singleSelectedId)
    : null;

  // We can transform either a part or an instance (but not joints)
  const hasTransformableSelection = selectedPart !== null || selectedInstance !== null;

  // Sync proxy position from IR when selection/document changes (but not during drag)
  useEffect(() => {
    if (!proxy || !hasTransformableSelection) return;
    if (isDraggingRef.current) return;

    if (selectedPart) {
      // Handle part transform — convert kernel Z-up to display Y-up
      const translateNode = document.nodes[String(selectedPart.translateNodeId)];
      const rotateNode = document.nodes[String(selectedPart.rotateNodeId)];
      const scaleNode = document.nodes[String(selectedPart.scaleNodeId)];

      if (translateNode?.op.type === "Translate") {
        proxy.position.set(...kernelToDisplay(translateNode.op.offset));
      }
      // Always set quaternion with coord rotation baked in
      const rotAngles =
        rotateNode?.op.type === "Rotate"
          ? rotateNode.op.angles
          : { x: 0, y: 0, z: 0 };
      kernelEulerToDisplayQuat(rotAngles, proxy.quaternion);
      if (scaleNode?.op.type === "Scale") {
        const { factor } = scaleNode.op;
        proxy.scale.set(factor.x, factor.y, factor.z);
      }
    } else if (selectedInstance) {
      // Handle instance transform — convert kernel Z-up to display Y-up
      const transform = selectedInstance.transform;
      const t = transform ?? {
        translation: { x: 0, y: 0, z: 0 },
        rotation: { x: 0, y: 0, z: 0 },
        scale: { x: 1, y: 1, z: 1 },
      };
      proxy.position.set(...kernelToDisplay(t.translation));
      kernelEulerToDisplayQuat(t.rotation, proxy.quaternion);
      proxy.scale.set(t.scale.x, t.scale.y, t.scale.z);
    }
  }, [proxy, selectedPart, selectedInstance, hasTransformableSelection, document]);

  // Handle dragging-changed event
  useEffect(() => {
    const controls = controlsRef.current;
    if (!controls) return;

    const onDraggingChanged = (event: { value: boolean }) => {
      const dragging = event.value;
      isDraggingRef.current = dragging;
      setDraggingGizmo(dragging);

      // Disable orbit controls during gizmo drag
      if (orbitControls.current) {
        orbitControls.current.enabled = !dragging;
      }

      if (dragging) {
        // Push undo snapshot at drag start
        useDocumentStore.getState().pushUndoSnapshot();
      }
    };

    controls.addEventListener("dragging-changed", onDraggingChanged);
    return () => {
      controls.removeEventListener("dragging-changed", onDraggingChanged);
    };
  }, [proxy, orbitControls, setDraggingGizmo]);

  // Handle transform changes during drag
  useEffect(() => {
    const controls = controlsRef.current;
    if (!controls) return;

    const onObjectChange = () => {
      if (!proxy || !singleSelectedId) return;

      const store = useDocumentStore.getState();

      if (selectedPart) {
        // Update part transform — convert display Y-up back to kernel Z-up
        if (transformMode === "translate") {
          store.setTranslation(
            singleSelectedId,
            displayToKernel(proxy.position),
            true, // skipUndo — we pushed at drag start
          );
        } else if (transformMode === "rotate") {
          store.setRotation(
            singleSelectedId,
            displayQuatToKernelEuler(proxy.quaternion),
            true,
          );
        } else if (transformMode === "scale") {
          store.setScale(
            singleSelectedId,
            { x: proxy.scale.x, y: proxy.scale.y, z: proxy.scale.z },
            true,
          );
        }
      } else if (selectedInstance) {
        // Update instance transform — convert display Y-up back to kernel Z-up
        store.setInstanceTransform(
          singleSelectedId,
          {
            translation: displayToKernel(proxy.position),
            rotation: displayQuatToKernelEuler(proxy.quaternion),
            scale: {
              x: proxy.scale.x,
              y: proxy.scale.y,
              z: proxy.scale.z,
            },
          },
          true, // skipUndo
        );
      }
    };

    controls.addEventListener("objectChange", onObjectChange);
    return () => {
      controls.removeEventListener("objectChange", onObjectChange);
    };
  }, [proxy, singleSelectedId, selectedPart, selectedInstance, transformMode]);

  if (!hasTransformableSelection) return null;

  const snapProps = gridSnap
    ? {
        translationSnap: snapIncrement,
        rotationSnap: (Math.PI / 180) * 15,
        scaleSnap: 0.1,
      }
    : {};

  return (
    <>
      <object3D ref={proxyCallbackRef} />
      {proxy && (
        <TransformControls
          ref={controlsRef}
          object={proxy}
          mode={transformMode}
          space="local"
          size={isCoarsePointer ? 1.8 : 0.8}
          {...snapProps}
        />
      )}
    </>
  );
}
