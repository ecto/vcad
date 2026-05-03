import { useXR } from "@react-three/xr";
import { xrStore, useXRSupportStore } from "@/stores/xr-store";

/**
 * Floating Enter-XR control. Renders nothing if neither VR nor AR is
 * supported, so desktop users never see it.
 */
export function EnterXRButton() {
  const checked = useXRSupportStore((s) => s.checked);
  const vr = useXRSupportStore((s) => s.vr);
  const ar = useXRSupportStore((s) => s.ar);
  // `mode` becomes a string (e.g. "immersive-vr") while a session is active.
  const mode = useXR((s) => s.mode);

  if (!checked || (!vr && !ar)) return null;
  const presenting = mode != null;

  if (presenting) {
    return (
      <button
        type="button"
        className="pointer-events-auto rounded-md border border-border/40 bg-bg/80 px-2 py-1 text-xs text-text shadow-sm backdrop-blur hover:bg-bg"
        onClick={() => xrStore.getState().session?.end()}
        title="Exit XR"
      >
        Exit XR
      </button>
    );
  }

  return (
    <div className="pointer-events-auto flex gap-1 rounded-md border border-border/40 bg-bg/80 p-1 shadow-sm backdrop-blur">
      {ar && (
        <button
          type="button"
          className="rounded px-2 py-1 text-xs text-text hover:bg-border/40"
          onClick={() => xrStore.enterAR()}
          title="Enter AR — passthrough on Vision Pro / Quest"
        >
          AR
        </button>
      )}
      {vr && (
        <button
          type="button"
          className="rounded px-2 py-1 text-xs text-text hover:bg-border/40"
          onClick={() => xrStore.enterVR()}
          title="Enter VR — fully immersive"
        >
          VR
        </button>
      )}
    </div>
  );
}
