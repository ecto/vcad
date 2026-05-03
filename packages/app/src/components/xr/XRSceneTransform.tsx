import { type ReactNode, useMemo } from "react";
import { useXRPresenting, useXRSupportStore } from "@/stores/xr-store";

/**
 * Mounts the kernel scene with the live XR view transform while a session is
 * active. The transform's scale + position are driven by `useXRSupportStore`
 * — initialized to 1/1000 (mm → m) at desktop height on entry, then updated
 * live by the bimanual scale-teleport gesture in `XRGestures`.
 *
 * Outside XR, this component is a passthrough so the existing orbit-camera
 * UX is untouched.
 */
export function XRSceneTransform({ children }: { children: ReactNode }) {
  const presenting = useXRPresenting();
  const view = useXRSupportStore((s) => s.view);

  // Memo the position tuple so the prop identity is stable for R3F's
  // reconciler when only `scale` changes.
  const position = useMemo<[number, number, number]>(
    () => [view.position[0], view.position[1], view.position[2]],
    [view.position],
  );

  if (!presenting) return <>{children}</>;
  return (
    <group scale={view.scale} position={position}>
      {children}
    </group>
  );
}
