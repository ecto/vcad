import { useEffect } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useEngineStore, useUiStore, useDocumentStore } from "@vcad/core";

export function useCameraControls() {
  const camera = useThree((s) => s.camera);
  const controls = useThree(
    (s) => s.controls as THREE.EventDispatcher & { target?: THREE.Vector3 } | null,
  );

  useEffect(() => {
    function handleFocusSelection() {
      const { selectedPartIds } = useUiStore.getState();
      if (selectedPartIds.size === 0) return;

      const scene = useEngineStore.getState().scene;
      const parts = useDocumentStore.getState().parts;
      if (!scene) return;

      // Compute bounding box from selected parts' meshes
      const box = new THREE.Box3();
      let found = false;
      for (const [idx, evalPart] of scene.parts.entries()) {
        const partInfo = parts[idx];
        if (!partInfo || !selectedPartIds.has(partInfo.id)) continue;

        const positions = evalPart.mesh.positions;
        for (let i = 0; i < positions.length; i += 3) {
          box.expandByPoint(
            new THREE.Vector3(positions[i], positions[i + 1], positions[i + 2]),
          );
        }
        found = true;
      }

      if (!found) return;

      const center = new THREE.Vector3();
      box.getCenter(center);
      const size = new THREE.Vector3();
      box.getSize(size);
      const maxDim = Math.max(size.x, size.y, size.z, 1);

      // Position camera to frame the bounding box
      const dist = maxDim * 2;
      const dir = new THREE.Vector3()
        .copy(camera.position)
        .sub(
          controls && "target" in controls && controls.target
            ? controls.target
            : new THREE.Vector3(0, 0, 0),
        )
        .normalize();

      camera.position.copy(center).addScaledVector(dir, dist);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(center);
      }
    }

    // Helper to get bounding box of all geometry in scene
    function getSceneBoundingBox(): THREE.Box3 | null {
      const scene = useEngineStore.getState().scene;
      if (!scene || scene.parts.length === 0) return null;

      const box = new THREE.Box3();
      for (const evalPart of scene.parts) {
        const positions = evalPart.mesh.positions;
        for (let i = 0; i < positions.length; i += 3) {
          box.expandByPoint(
            new THREE.Vector3(positions[i], positions[i + 1], positions[i + 2]),
          );
        }
      }
      return box.isEmpty() ? null : box;
    }

    function handleCameraIsometric() {
      const box = getSceneBoundingBox();
      const center = new THREE.Vector3();
      let dist = 150;

      if (box) {
        box.getCenter(center);
        const size = new THREE.Vector3();
        box.getSize(size);
        dist = Math.max(size.x, size.y, size.z, 50) * 2;
      }

      // Isometric: equal angles from all axes
      const offset = dist / Math.sqrt(3);
      camera.position.set(center.x + offset, center.y + offset, center.z + offset);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(center);
      }
    }

    function handleCameraFit() {
      const box = getSceneBoundingBox();
      const center = new THREE.Vector3();
      let dist = 150;

      if (box) {
        box.getCenter(center);
        const size = new THREE.Vector3();
        box.getSize(size);
        dist = Math.max(size.x, size.y, size.z, 50) * 2;
      }

      // Preserve current direction when there's already a meaningful target;
      // otherwise fall back to an isometric angle so pressing Fit on an empty
      // scene still moves the camera somewhere sensible.
      const currentTarget =
        controls && "target" in controls && controls.target
          ? controls.target
          : new THREE.Vector3(0, 0, 0);
      let dir = new THREE.Vector3().copy(camera.position).sub(currentTarget);
      if (dir.lengthSq() < 1e-6) {
        dir.set(1, 1, 1);
      }
      dir.normalize();

      camera.position.copy(center).addScaledVector(dir, dist);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(center);
      }
    }

    function handleCameraTop() {
      const box = getSceneBoundingBox();
      const center = new THREE.Vector3();
      let dist = 150;

      if (box) {
        box.getCenter(center);
        const size = new THREE.Vector3();
        box.getSize(size);
        dist = Math.max(size.x, size.y, size.z, 50) * 2;
      }

      // Top: looking down -Z
      camera.position.set(center.x, center.y, center.z + dist);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(center);
      }
    }

    function handleCameraFront() {
      const box = getSceneBoundingBox();
      const center = new THREE.Vector3();
      let dist = 150;

      if (box) {
        box.getCenter(center);
        const size = new THREE.Vector3();
        box.getSize(size);
        dist = Math.max(size.x, size.y, size.z, 50) * 2;
      }

      // Front: looking down -Y
      camera.position.set(center.x, center.y - dist, center.z);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(center);
      }
    }

    function handleCameraRight() {
      const box = getSceneBoundingBox();
      const center = new THREE.Vector3();
      let dist = 150;

      if (box) {
        box.getCenter(center);
        const size = new THREE.Vector3();
        box.getSize(size);
        dist = Math.max(size.x, size.y, size.z, 50) * 2;
      }

      // Right: looking down -X
      camera.position.set(center.x + dist, center.y, center.z);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(center);
      }
    }

    window.addEventListener("vcad:focus-selection", handleFocusSelection);
    window.addEventListener("vcad:camera-isometric", handleCameraIsometric);
    window.addEventListener("vcad:camera-fit", handleCameraFit);
    window.addEventListener("vcad:camera-top", handleCameraTop);
    window.addEventListener("vcad:camera-front", handleCameraFront);
    window.addEventListener("vcad:camera-right", handleCameraRight);

    return () => {
      window.removeEventListener("vcad:focus-selection", handleFocusSelection);
      window.removeEventListener("vcad:camera-isometric", handleCameraIsometric);
      window.removeEventListener("vcad:camera-fit", handleCameraFit);
      window.removeEventListener("vcad:camera-top", handleCameraTop);
      window.removeEventListener("vcad:camera-front", handleCameraFront);
      window.removeEventListener("vcad:camera-right", handleCameraRight);
    };
  }, [camera, controls]);
}
