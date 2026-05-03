import { type ReactNode, useMemo } from "react";
import { useXRPresenting } from "@/stores/xr-store";

/**
 * Mounts the kernel scene at "desktop scale" while a WebXR session is active.
 *
 * Kernel coordinates are millimeters (1 unit = 1 mm). WebXR works in meters.
 * A scale factor of 1/1000 makes the model appear at true physical size in
 * the headset — a 100 mm cube reads as a 10 cm cube on your desk. The model
 * is anchored at headset-height (1 m) and 0.7 m forward so it floats just in
 * front of where the user is sitting on entry.
 *
 * Outside XR, this component is a passthrough so the existing orbit-camera
 * UX is untouched.
 */
const XR_SCALE = 0.001;
const XR_POSITION: [number, number, number] = [0, 1.0, -0.7];

export function XRSceneTransform({ children }: { children: ReactNode }) {
  const presenting = useXRPresenting();

  // Memo so the prop identity is stable; otherwise R3F reconciles the group
  // every render and forces children to remount.
  const transform = useMemo(
    () => ({
      scale: presenting ? XR_SCALE : 1,
      position: presenting ? XR_POSITION : ([0, 0, 0] as [number, number, number]),
    }),
    [presenting],
  );

  if (!presenting) return <>{children}</>;
  return (
    <group scale={transform.scale} position={transform.position}>
      {children}
    </group>
  );
}
