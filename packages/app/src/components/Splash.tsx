import { useEffect, useState } from "react";
import { useBootStore, type BootPhase } from "@/stores/boot-store";

const FADE_MS = 300;
const APP_VERSION = __APP_VERSION__;

/**
 * Tips of the day. One is picked at random on each splash mount, so a
 * boot becomes a small chance to surface a feature or a bit of
 * personality. Keep each line under ~70 chars so it fits on one row.
 */
const TIPS: readonly string[] = [
  "Press ⌘K anywhere for the command palette.",
  "Drag a .step file into the viewport to import it.",
  "Sketches support parallel, tangent, and equal-length constraints.",
  "Ray-traced mode renders BRep directly — no tessellation artifacts.",
  "Every parameter is scrubbable. Click, drag, watch it rebuild.",
  "Export to STL, GLB, or STEP from the File menu.",
  "Physics via phyz. Build a robot, give it joints, watch it move.",
  "Booleans run on exact BRep, not triangle-mesh hacks.",
  "Ask the AI chat to build geometry. It has real CAD tools.",
  "Shell hollows a solid to a given wall thickness.",
  "Undo and redo — experiment freely.",
  "Patterns stay linked to their source. Edit once, update everywhere.",
  "Joints: revolute, prismatic, cylindrical, ball, fixed.",
  "Drafting views project your parts to 2D orthographic drawings.",
  "Rust compiled to WebAssembly, running in your browser.",
  "Z is up. CAD convention, not Three.js.",
  "Shewchuk's exact predicates power the boolean face classifier.",
  "Every .vcad file is plain JSON — inspect it in any text editor.",
  "Sign in with Google or GitHub to sync across devices.",
  "Half-edge topology. Full-edge respect.",
  "Built by hand. Rendered by math.",
  "Open source. Free for everyone. Always.",
];

function pickTip(): string {
  return TIPS[Math.floor(Math.random() * TIPS.length)] ?? TIPS[0]!;
}

/**
 * Full-screen boot splash. Owns the pre-render UI while `bootstrap()`
 * walks through its phases, then fades itself out and unmounts so React
 * Three Fiber never mounts over a half-initialized engine.
 */
export function Splash() {
  const phase = useBootStore((s) => s.phase);
  const bytesReceived = useBootStore((s) => s.bytesReceived);
  const bytesTotal = useBootStore((s) => s.bytesTotal);
  const slowNetwork = useBootStore((s) => s.slowNetwork);

  const [fadingOut, setFadingOut] = useState(false);
  const [mounted, setMounted] = useState(true);
  const [tip] = useState(pickTip);

  useEffect(() => {
    if (phase !== "ready") return;
    setFadingOut(true);
    const unmountTimer = setTimeout(() => setMounted(false), FADE_MS);
    return () => clearTimeout(unmountTimer);
  }, [phase]);

  if (!mounted) return null;

  const label = phaseLabel(phase, bytesReceived, bytesTotal);
  const indeterminate =
    phase === "starting-engine" ||
    phase === "loading-document" ||
    phase === "evaluating" ||
    (phase === "fetching-kernel" && bytesTotal === 0);

  const progressPct =
    !indeterminate && bytesTotal > 0
      ? Math.min(100, Math.round((bytesReceived / bytesTotal) * 100))
      : 0;

  return (
    <div
      className={`fixed inset-0 z-[100] flex items-center justify-center bg-bg transition-opacity duration-300 ${
        fadingOut ? "opacity-0" : "opacity-100"
      }`}
      aria-busy={phase !== "ready"}
      aria-live="polite"
    >
      <div className="flex w-[260px] max-w-[92vw] flex-col items-center gap-5 text-center">
        <div className="flex flex-col items-center gap-2">
          <div className="text-6xl font-bold tracking-tighter text-text select-none leading-none">
            vcad<span className="text-brand">.</span>
          </div>
          <div className="text-xs text-text-muted">
            Free parametric CAD for everyone
          </div>
        </div>

        <div className="flex w-full flex-col gap-2">
          <ProgressBar
            progressPct={progressPct}
            indeterminate={indeterminate}
          />
          <div className="flex justify-between text-[11px] text-text-muted tabular-nums">
            <span>{label}</span>
            <span className="opacity-60">v{APP_VERSION}</span>
          </div>
          {slowNetwork && phase === "fetching-kernel" && (
            <div className="text-[11px] text-text-muted opacity-70">
              Slow connection — this caches after first load, future visits
              are instant.
            </div>
          )}
        </div>

        <div className="text-[10px] text-text-muted opacity-60 italic">
          {tip}
        </div>
      </div>
    </div>
  );
}

function ProgressBar({
  progressPct,
  indeterminate,
}: {
  progressPct: number;
  indeterminate: boolean;
}) {
  return (
    <div className="relative h-[3px] w-full overflow-hidden rounded-full bg-border">
      {indeterminate ? (
        <div className="absolute inset-y-0 w-1/3 animate-[splash-slide_1.2s_ease-in-out_infinite] rounded-full bg-brand" />
      ) : (
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-brand transition-[width] duration-150 ease-out"
          style={{ width: `${progressPct}%` }}
        />
      )}
      <style>{`
        @keyframes splash-slide {
          0% { left: -33%; }
          50% { left: 50%; }
          100% { left: 100%; }
        }
      `}</style>
    </div>
  );
}

function phaseLabel(
  phase: BootPhase,
  received: number,
  total: number,
): string {
  switch (phase) {
    case "fetching-kernel": {
      if (total > 0) {
        const mb = (received / 1_000_000).toFixed(1);
        const totalMb = (total / 1_000_000).toFixed(1);
        return `Loading kernel… ${mb} / ${totalMb} MB`;
      }
      if (received > 0) {
        return `Loading kernel… ${(received / 1_000_000).toFixed(1)} MB`;
      }
      return "Loading kernel…";
    }
    case "starting-engine":
      return "Starting engine…";
    case "loading-document":
      return "Loading document…";
    case "evaluating":
      return "Evaluating scene…";
    case "ready":
      return "Ready";
  }
}
