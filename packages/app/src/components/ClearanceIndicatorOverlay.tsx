import { Line, Html } from "@react-three/drei";
import { useUiStore } from "@vcad/core";

/**
 * Min-distance witness line for a chat `check_clearance` result: the segment
 * between the closest pair of points on the two measured groups, with the
 * measured distance and pass/fail verdict. Rendered inside the kernel-space
 * (Z-up) scene group like the other measurement overlays; suppressed during
 * AI screenshot capture so tool screenshots show geometry, not chrome.
 */
export function ClearanceIndicatorOverlay() {
  const indicator = useUiStore((s) => s.clearanceIndicator);
  const captureMode = useUiStore((s) => s.captureMode);
  if (!indicator || captureMode) return null;

  const { pointA, pointB, distanceMm, pass, label } = indicator;
  const mid: [number, number, number] = [
    (pointA[0] + pointB[0]) / 2,
    (pointA[1] + pointB[1]) / 2,
    (pointA[2] + pointB[2]) / 2,
  ];
  const color = pass ? "#22c55e" : "#ef4444";

  return (
    <group>
      <Line points={[pointA, pointB]} color={color} lineWidth={2} dashed dashSize={2} gapSize={1.2} />
      <mesh position={pointA}>
        <sphereGeometry args={[0.8, 12, 12]} />
        <meshBasicMaterial color={color} />
      </mesh>
      <mesh position={pointB}>
        <sphereGeometry args={[0.8, 12, 12]} />
        <meshBasicMaterial color={color} />
      </mesh>
      <Html position={mid} center zIndexRange={[40, 0]}>
        <div
          className="pointer-events-auto flex items-center gap-1.5 rounded border px-1.5 py-0.5 text-[10px] font-mono whitespace-nowrap shadow-sm"
          style={{
            background: "var(--color-bg, #fff)",
            borderColor: color,
            color,
          }}
        >
          <span>
            {label ? `${label}: ` : ""}
            {distanceMm.toFixed(2)} mm
          </span>
          <span className="font-sans">{pass ? "clear" : "violated"}</span>
          <button
            aria-label="Dismiss clearance measurement"
            className="ml-0.5 opacity-60 hover:opacity-100"
            onClick={() => useUiStore.getState().setClearanceIndicator(null)}
          >
            ×
          </button>
        </div>
      </Html>
    </group>
  );
}
