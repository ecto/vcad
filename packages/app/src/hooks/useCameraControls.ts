import { useEffect, useRef } from "react";
import { useThree } from "@react-three/fiber";
import * as THREE from "three";
import { useEngineStore, useUiStore, useDocumentStore } from "@vcad/core";

export function useCameraControls() {
  const camera = useThree((s) => s.camera);
  const controls = useThree(
    (s) => s.controls as THREE.EventDispatcher & { target?: THREE.Vector3 } | null,
  );
  const fittedForDocRef = useRef<string | null>(null);

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
      if (!scene) return null;

      const box = new THREE.Box3();
      const tmp = new THREE.Vector3();

      // Assembly mode: instances carry their own world transform from FK,
      // so the link-local mesh positions need to be transformed before
      // they go into the bbox. Skip scene.parts in this mode — for URDF
      // assemblies the importer populates both lists, but parts hold the
      // un-articulated link-local geometry which would collapse the bbox
      // around the origin and break auto-fit.
      if (scene.instances && scene.instances.length > 0) {
        for (const inst of scene.instances) {
          const t = inst.transform;
          const positions = inst.mesh.positions;
          for (let i = 0; i < positions.length; i += 3) {
            const px = positions[i]!;
            const py = positions[i + 1]!;
            const pz = positions[i + 2]!;
            if (t) {
              tmp.set(
                px * t.scale.x + t.translation.x,
                py * t.scale.y + t.translation.y,
                pz * t.scale.z + t.translation.z,
              );
            } else {
              tmp.set(px, py, pz);
            }
            box.expandByPoint(tmp);
          }
        }
      } else {
        if (scene.parts.length === 0) return null;
        for (const evalPart of scene.parts) {
          const positions = evalPart.mesh.positions;
          for (let i = 0; i < positions.length; i += 3) {
            box.expandByPoint(
              new THREE.Vector3(positions[i], positions[i + 1], positions[i + 2]),
            );
          }
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
      const kernelCenter = new THREE.Vector3();
      let dist = 150;

      if (box) {
        box.getCenter(kernelCenter);
        const size = new THREE.Vector3();
        box.getSize(size);
        dist = Math.max(size.x, size.y, size.z, 50) * 2;
      }

      // The bounding box is in kernel (Z-up) space because the meshes are
      // stored that way and rendered through the -90°X rotation group. The
      // camera and OrbitControls live outside that rotation group, so we
      // need the display-space (Y-up) center: (x, y, z) → (x, z, -y).
      const displayCenter = new THREE.Vector3(
        kernelCenter.x,
        kernelCenter.z,
        -kernelCenter.y,
      );

      // Preserve current direction when there's already a meaningful target;
      // otherwise fall back to an isometric angle so pressing Fit on an empty
      // scene still moves the camera somewhere sensible.
      const currentTarget =
        controls && "target" in controls && controls.target
          ? controls.target
          : new THREE.Vector3(0, 0, 0);
      const dir = new THREE.Vector3().copy(camera.position).sub(currentTarget);
      if (dir.lengthSq() < 1e-6) {
        dir.set(1, 1, 1);
      }
      dir.normalize();

      camera.position.copy(displayCenter).addScaledVector(dir, dist);
      if (controls && "target" in controls && controls.target) {
        controls.target.copy(displayCenter);
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
    // `vcad:camera-fit` is handled by ViewportContent's animated variant, so
    // double-click / View → Fit fly into place. Boot's auto-fit (below) calls
    // handleCameraFit() directly and stays as a snap so the user doesn't
    // watch a sweep on every reload.
    window.addEventListener("vcad:camera-top", handleCameraTop);
    window.addEventListener("vcad:camera-front", handleCameraFront);
    window.addEventListener("vcad:camera-right", handleCameraRight);

    // Auto-fit on document load: subscribe to (documentId, scene) inside R3F
    // so camera + controls are guaranteed mounted by the time the fit runs.
    // Keyed by documentId so opening a new doc refits but in-doc edits don't.
    // Skipped when `?at=` is present (share URL with captured viewer state).
    const tryAutoFit = () => {
      // Wait until OrbitControls is actually mounted — `controls` arrives a
      // tick after `camera` because the OrbitControls component mounts as a
      // child of Canvas. Without it, the fit would land on the camera but
      // OrbitControls would re-target to its old (default) target on the
      // next frame, snapping the view back.
      if (!controls || !("target" in controls) || !controls.target) return;
      if (typeof window !== "undefined") {
        const params = new URLSearchParams(window.location.search);
        if (params.has("at")) return;
      }
      const docId = useDocumentStore.getState().documentId;
      const scene = useEngineStore.getState().scene;
      if (!docId) return;
      if (fittedForDocRef.current === docId) return;
      const hasGeom =
        !!scene &&
        (scene.parts.some((p) => p.mesh.indices.length > 0) ||
          (scene.instances?.some((i) => i.mesh.indices.length > 0) ?? false));
      if (!hasGeom) return;
      fittedForDocRef.current = docId;
      handleCameraFit();
    };
    tryAutoFit();
    const unsubDoc = useDocumentStore.subscribe(tryAutoFit);
    const unsubEng = useEngineStore.subscribe(tryAutoFit);

    return () => {
      window.removeEventListener("vcad:focus-selection", handleFocusSelection);
      window.removeEventListener("vcad:camera-isometric", handleCameraIsometric);
      window.removeEventListener("vcad:camera-top", handleCameraTop);
      window.removeEventListener("vcad:camera-front", handleCameraFront);
      window.removeEventListener("vcad:camera-right", handleCameraRight);
      unsubDoc();
      unsubEng();
    };
  }, [camera, controls]);
}
