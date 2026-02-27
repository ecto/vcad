/**
 * PCB camera controller hook.
 *
 * Phase 1: Orthographic camera looking down -Y, orbit disabled, pan+zoom enabled.
 * Phase 2: Middle-mouse vertical drag tilts the camera into 3D.
 *   - tiltAngle < 5deg: orbit locked, pure pan+zoom (familiar 2D editing)
 *   - tiltAngle >= 5deg: orbit unlocked, polar angle constrained to ~75deg max
 *   - Smooth animated transition via lerp
 *   - Ortho camera used throughout (parallel projection stays clean even when tilted)
 */

import { useEffect, useRef, useCallback } from "react";
import { useThree, useFrame } from "@react-three/fiber";
import { OrthographicCamera, Vector3, Spherical, MathUtils } from "three";
import type { OrbitControls as OrbitControlsImpl } from "three-stdlib";
import { useElectronicsStore } from "@/stores/electronics-store";

const PCB_CAMERA_HEIGHT = 200;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 100;
const TILT_THRESHOLD_DEG = 5;
const MAX_TILT_DEG = 75;
const TILT_SENSITIVITY = 0.3; // degrees per pixel of mouse movement

// Reusable temporaries
const _spherical = new Spherical();
const _offset = new Vector3();

export function usePcbCamera(
  orbitRef: React.RefObject<OrbitControlsImpl | null>,
  active: boolean,
) {
  const { camera, gl, invalidate, set } = useThree();
  const orthoCamRef = useRef<OrthographicCamera | null>(null);
  const prevCameraRef = useRef<typeof camera | null>(null);

  // Tilt drag tracking
  const tiltDragRef = useRef<{ startY: number; startAngle: number } | null>(null);

  // Animation target for smooth tilt transitions
  const tiltGoalRef = useRef(0);
  const isAnimatingTiltRef = useRef(false);

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
    const frustumSize = 60;

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
    orthoCam.position.set(25, PCB_CAMERA_HEIGHT, -15);
    orthoCam.up.set(0, 0, -1);
    orthoCam.lookAt(25, 0, -15);
    orthoCam.zoom = 1;
    orthoCam.updateProjectionMatrix();

    set({ camera: orthoCam });
    invalidate();

    // Reset tilt
    useElectronicsStore.getState().setTiltAngle(0);
    tiltGoalRef.current = 0;

    // Configure OrbitControls for PCB mode
    const controls = orbitRef.current;
    if (controls) {
      controls.object = orthoCam;
      controls.target.set(25, 0, -15);
      controls.enableRotate = false;
      controls.enableZoom = false;
      controls.enablePan = true;
      controls.screenSpacePanning = true;
      controls.mouseButtons = {
        LEFT: undefined,
        MIDDLE: 2 as number, // PAN — we intercept for tilt via pointerdown
        RIGHT: 2 as number,
      };
      controls.update();
    }

    return () => {
      const ctrl = orbitRef.current;
      if (ctrl && prevCameraRef.current) {
        ctrl.object = prevCameraRef.current;
        ctrl.enableRotate = true;
        ctrl.enableZoom = false;
        ctrl.update();
      }
    };
  }, [active, gl, set, invalidate]);

  // Position camera at the given tilt angle around the orbit target
  const applyCameraTilt = useCallback((angleDeg: number) => {
    const orthoCam = orthoCamRef.current;
    const controls = orbitRef.current;
    if (!orthoCam || !controls) return;

    const target = controls.target;
    const angleRad = MathUtils.degToRad(angleDeg);

    // Current distance from target
    _offset.subVectors(orthoCam.position, target);
    const distance = _offset.length();

    // Compute new position: rotate from top-down (-Y direction) by tilt angle
    // In display space: top-down is along -Y. Tilting rotates toward +Z (toward viewer).
    // Spherical: phi=0 is +Y, phi=PI/2 is XZ plane
    // Top-down looking at -Y: camera at +Y relative to target => phi=0
    // Tilted: phi increases as we tilt
    const phi = angleRad; // 0 = top-down, PI/2 = horizon
    const theta = 0; // Keep azimuth at 0 (looking from +Z side in display space)

    _spherical.set(distance, phi, theta);
    _offset.setFromSpherical(_spherical);

    orthoCam.position.copy(target).add(_offset);

    // Update up vector: transition from (0,0,-1) at top-down to (0,1,0) when tilted
    const t = Math.min(angleDeg / 15, 1); // smooth transition over first 15 degrees
    orthoCam.up.set(0, t, -(1 - t)).normalize();

    orthoCam.lookAt(target);
    orthoCam.updateProjectionMatrix();
    controls.update();
  }, []);

  // Middle-mouse drag for tilt control
  useEffect(() => {
    if (!active) return;
    const canvas = gl.domElement;

    const handlePointerDown = (e: PointerEvent) => {
      // Middle mouse button for tilt
      if (e.button !== 1) return;

      // Only intercept if shift is held (shift+MMB = tilt, plain MMB = pan)
      if (!e.shiftKey) return;

      e.preventDefault();
      e.stopPropagation();

      const currentAngle = useElectronicsStore.getState().tiltAngle;
      tiltDragRef.current = { startY: e.clientY, startAngle: currentAngle };

      // Temporarily disable OrbitControls pan so it doesn't fight
      const controls = orbitRef.current;
      if (controls) controls.enablePan = false;
    };

    const handlePointerMove = (e: PointerEvent) => {
      if (!tiltDragRef.current) return;

      const dy = tiltDragRef.current.startY - e.clientY; // up = positive tilt
      const newAngle = Math.max(0, Math.min(MAX_TILT_DEG, tiltDragRef.current.startAngle + dy * TILT_SENSITIVITY));

      useElectronicsStore.getState().setTiltAngle(newAngle);
      applyCameraTilt(newAngle);
      invalidate();
    };

    const handlePointerUp = (_e: PointerEvent) => {
      if (!tiltDragRef.current) return;
      tiltDragRef.current = null;

      // Re-enable pan
      const controls = orbitRef.current;
      if (controls) controls.enablePan = true;

      // Snap to flat if below threshold
      const angle = useElectronicsStore.getState().tiltAngle;
      if (angle < TILT_THRESHOLD_DEG && angle > 0) {
        tiltGoalRef.current = 0;
        isAnimatingTiltRef.current = true;
      }
    };

    // Also handle 'E' key for stackup explosion toggle
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "e" || e.key === "E") {
        // Don't trigger if user is typing in an input
        if ((e.target as HTMLElement)?.tagName === "INPUT" || (e.target as HTMLElement)?.tagName === "TEXTAREA") return;
        useElectronicsStore.getState().toggleStackupExplosion();
        invalidate();
      }
    };

    canvas.addEventListener("pointerdown", handlePointerDown, { capture: true });
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("keydown", handleKeyDown);

    return () => {
      canvas.removeEventListener("pointerdown", handlePointerDown, { capture: true });
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [active, gl, invalidate, applyCameraTilt]);

  // Smooth tilt animation (snap back to 0 when below threshold)
  useFrame(() => {
    if (!active || !isAnimatingTiltRef.current) return;

    const current = useElectronicsStore.getState().tiltAngle;
    const goal = tiltGoalRef.current;
    const diff = goal - current;

    if (Math.abs(diff) < 0.1) {
      // Done
      useElectronicsStore.getState().setTiltAngle(goal);
      applyCameraTilt(goal);
      isAnimatingTiltRef.current = false;
    } else {
      const next = current + diff * 0.15;
      useElectronicsStore.getState().setTiltAngle(next);
      applyCameraTilt(next);
      invalidate();
    }
  });

  // Handle ortho zoom-to-cursor via wheel
  useEffect(() => {
    if (!active) return;
    const canvas = gl.domElement;

    const handleWheel = (e: WheelEvent) => {
      e.preventDefault();

      const orthoCam = orthoCamRef.current;
      if (!orthoCam) return;

      let dy = e.deltaY;
      if (e.deltaMode === 1) dy *= 16;
      if (e.deltaMode === 2) dy *= 100;

      if (e.ctrlKey || e.metaKey || e.deltaMode === 1 || Math.abs(e.deltaX) < Math.abs(e.deltaY) * 0.5) {
        // Zoom
        const zoomDelta = dy > 0 ? 0.9 : 1.1;
        const newZoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, orthoCam.zoom * zoomDelta));

        if (newZoom !== orthoCam.zoom) {
          const rect = canvas.getBoundingClientRect();
          const ndcX = ((e.clientX - rect.left) / rect.width) * 2 - 1;
          const ndcY = -((e.clientY - rect.top) / rect.height) * 2 + 1;

          const frustumW = (orthoCam.right - orthoCam.left) / orthoCam.zoom;
          const frustumH = (orthoCam.top - orthoCam.bottom) / orthoCam.zoom;
          const worldX = orthoCam.position.x + ndcX * frustumW / 2;
          const worldZ = orthoCam.position.z - ndcY * frustumH / 2;

          orthoCam.zoom = newZoom;
          orthoCam.updateProjectionMatrix();

          const newFrustumW = (orthoCam.right - orthoCam.left) / orthoCam.zoom;
          const newFrustumH = (orthoCam.top - orthoCam.bottom) / orthoCam.zoom;
          const newWorldX = orthoCam.position.x + ndcX * newFrustumW / 2;
          const newWorldZ = orthoCam.position.z - ndcY * newFrustumH / 2;

          const dx = worldX - newWorldX;
          const dz = worldZ - newWorldZ;
          orthoCam.position.x += dx;
          orthoCam.position.z += dz;

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
