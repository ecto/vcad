/**
 * PCB camera controller hook.
 *
 * Configures the R3F camera for PCB editing:
 * - Orthographic camera looking down -Y (after rotation group: kernel Z -> display Y)
 * - Orbit disabled (pure pan + zoom)
 * - Zoom-to-cursor adjusts ortho frustum
 * - Phase 2 will add tilt gesture to unlock orbit
 */

import { useEffect, useRef } from "react";
import { useThree, useFrame } from "@react-three/fiber";
import { OrthographicCamera } from "three";
import type { OrbitControls as OrbitControlsImpl } from "three-stdlib";

const PCB_CAMERA_HEIGHT = 200;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 100;

export function usePcbCamera(
  orbitRef: React.RefObject<OrbitControlsImpl | null>,
  active: boolean,
) {
  const { camera, gl, invalidate, set } = useThree();
  const orthoCamRef = useRef<OrthographicCamera | null>(null);
  const prevCameraRef = useRef<typeof camera | null>(null);

  // Create orthographic camera and swap it in when PCB mode activates
  useEffect(() => {
    if (!active) {
      // Restore previous camera
      if (prevCameraRef.current) {
        set({ camera: prevCameraRef.current });
        prevCameraRef.current = null;
      }
      return;
    }

    // Save current camera for restoration
    prevCameraRef.current = camera;

    // Create or reuse ortho camera
    const canvas = gl.domElement;
    const aspect = canvas.clientWidth / canvas.clientHeight;
    const frustumSize = 60; // mm visible vertically at zoom=1

    let orthoCam = orthoCamRef.current;
    if (!orthoCam) {
      orthoCam = new OrthographicCamera(
        -frustumSize * aspect / 2,
        frustumSize * aspect / 2,
        frustumSize / 2,
        -frustumSize / 2,
        0.1,
        2000,
      );
      orthoCamRef.current = orthoCam;
    }

    // Position camera above board center, looking down -Y (display space)
    // After rotation group: kernel Z-up -> display Y-up, so looking down -Y = looking down kernel -Z
    orthoCam.position.set(25, PCB_CAMERA_HEIGHT, -15);
    orthoCam.up.set(0, 0, -1); // PCB Y axis points into screen (-Z in display)
    orthoCam.lookAt(25, 0, -15);
    orthoCam.zoom = 1;
    orthoCam.updateProjectionMatrix();

    set({ camera: orthoCam });
    invalidate();

    // Configure OrbitControls for PCB mode
    const controls = orbitRef.current;
    if (controls) {
      controls.object = orthoCam;
      controls.target.set(25, 0, -15);
      controls.enableRotate = false; // No orbit in Phase 1
      controls.enableZoom = false; // We handle zoom ourselves for zoom-to-cursor
      controls.enablePan = true;
      controls.screenSpacePanning = true;
      controls.mouseButtons = {
        LEFT: undefined, // Reserved for tools
        MIDDLE: 2 as number, // PAN
        RIGHT: 2 as number, // PAN
      };
      controls.update();
    }

    return () => {
      // Restore orbit controls settings for 3D mode
      const ctrl = orbitRef.current;
      if (ctrl && prevCameraRef.current) {
        ctrl.object = prevCameraRef.current;
        ctrl.enableRotate = true;
        ctrl.enableZoom = false; // ViewportContent handles zoom
        ctrl.update();
      }
    };
  }, [active, gl, set, invalidate]);

  // Handle ortho zoom-to-cursor via wheel
  useEffect(() => {
    if (!active) return;

    const canvas = gl.domElement;

    const handleWheel = (e: WheelEvent) => {
      e.preventDefault();

      const orthoCam = orthoCamRef.current;
      if (!orthoCam) return;

      // Normalize delta
      let dy = e.deltaY;
      if (e.deltaMode === 1) dy *= 16;
      if (e.deltaMode === 2) dy *= 100;

      if (e.ctrlKey || e.metaKey || e.deltaMode === 1 || Math.abs(e.deltaX) < Math.abs(e.deltaY) * 0.5) {
        // Zoom
        const zoomDelta = dy > 0 ? 0.9 : 1.1;
        const newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, orthoCam.zoom * zoomDelta));

        if (newZoom !== orthoCam.zoom) {
          // Zoom toward cursor
          const rect = canvas.getBoundingClientRect();
          const ndcX = ((e.clientX - rect.left) / rect.width) * 2 - 1;
          const ndcY = -((e.clientY - rect.top) / rect.height) * 2 + 1;

          // World point under cursor before zoom
          const frustumW = (orthoCam.right - orthoCam.left) / orthoCam.zoom;
          const frustumH = (orthoCam.top - orthoCam.bottom) / orthoCam.zoom;
          const worldX = orthoCam.position.x + ndcX * frustumW / 2;
          const worldZ = orthoCam.position.z - ndcY * frustumH / 2;

          orthoCam.zoom = newZoom;
          orthoCam.updateProjectionMatrix();

          // World point under cursor after zoom
          const newFrustumW = (orthoCam.right - orthoCam.left) / orthoCam.zoom;
          const newFrustumH = (orthoCam.top - orthoCam.bottom) / orthoCam.zoom;
          const newWorldX = orthoCam.position.x + ndcX * newFrustumW / 2;
          const newWorldZ = orthoCam.position.z - ndcY * newFrustumH / 2;

          // Shift camera to keep cursor point fixed
          const dx = worldX - newWorldX;
          const dz = worldZ - newWorldZ;
          orthoCam.position.x += dx;
          orthoCam.position.z += dz;

          // Update orbit controls target
          const controls = orbitRef.current;
          if (controls) {
            controls.target.x += dx;
            controls.target.z += dz;
            controls.update();
          }
        }
      } else {
        // Pan (trackpad two-finger scroll)
        const controls = orbitRef.current;
        if (!controls) return;
        const frustumW = (orthoCam.right - orthoCam.left) / orthoCam.zoom;
        const panScale = frustumW / canvas.clientWidth;
        orthoCam.position.x += e.deltaX * panScale;
        orthoCam.position.z += e.deltaY * panScale;
        controls.target.x += e.deltaX * panScale;
        controls.target.z += e.deltaY * panScale;
        controls.update();
      }

      orthoCam.updateProjectionMatrix();
      invalidate();
    };

    canvas.addEventListener("wheel", handleWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", handleWheel);
  }, [active, gl, invalidate]);

  // Keep ortho frustum in sync with canvas resize
  useFrame(() => {
    if (!active) return;
    const orthoCam = orthoCamRef.current;
    if (!orthoCam) return;

    const canvas = gl.domElement;
    const aspect = canvas.clientWidth / canvas.clientHeight;
    const frustumSize = 60;

    const newLeft = -frustumSize * aspect / 2;
    const newRight = frustumSize * aspect / 2;
    if (Math.abs(orthoCam.left - newLeft) > 0.01) {
      orthoCam.left = newLeft;
      orthoCam.right = newRight;
      orthoCam.top = frustumSize / 2;
      orthoCam.bottom = -frustumSize / 2;
      orthoCam.updateProjectionMatrix();
    }
  });

  return orthoCamRef;
}
