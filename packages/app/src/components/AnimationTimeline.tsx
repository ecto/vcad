/**
 * Animation transport bar for the document timeline (`doc.timeline`).
 *
 * A summoned bottom-center surface: appears when the open document carries
 * an animation timeline (authored via the MCP `animate` tool). Play/pause,
 * scrub, loop, and speed — playback itself lives in `useTimelinePlayback`
 * inside the canvas and never mutates the document.
 */

import { useEffect } from "react";
import { Pause } from "@phosphor-icons/react/dist/ssr/Pause";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { Repeat } from "@phosphor-icons/react/dist/ssr/Repeat";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { FilmStrip } from "@phosphor-icons/react/dist/ssr/FilmStrip";
import { useDocumentStore } from "@vcad/core";
import { useAnimationStore } from "@/stores/animation-store";
import { hasJointTracks } from "@/lib/timeline-sample";
import { cn } from "@/lib/utils";

const SPEEDS = [0.25, 0.5, 1, 2, 4];

function formatTime(s: number): string {
  return `${s.toFixed(2)}s`;
}

export function AnimationTimeline() {
  const timeline = useDocumentStore((s) => s.document.timeline);
  const visible = useAnimationStore((s) => s.visible);
  const playing = useAnimationStore((s) => s.playing);
  const timeS = useAnimationStore((s) => s.timeS);
  const loop = useAnimationStore((s) => s.loop);
  const speed = useAnimationStore((s) => s.speed);

  const playable = hasJointTracks(timeline);

  // Auto-summon the transport when a document with an animatable timeline
  // loads; retract it when the timeline goes away.
  useEffect(() => {
    const anim = useAnimationStore.getState();
    if (playable && !anim.visible) {
      anim.show();
    } else if (!playable && anim.visible) {
      anim.hide();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- summon on playability changes only
  }, [playable]);

  if (!visible || !timeline || !playable) return null;

  const duration = Math.max(timeline.durationS, 1e-6);
  const anim = useAnimationStore.getState();

  return (
    <div
      className={cn(
        "absolute bottom-10 left-1/2 z-30 -translate-x-1/2",
        "flex items-center gap-3 rounded-lg border border-border bg-surface/95 px-3 py-2 shadow-lg backdrop-blur",
      )}
      data-testid="animation-timeline"
    >
      <FilmStrip size={14} className="shrink-0 text-text-muted" />
      <button
        type="button"
        onClick={anim.togglePlay}
        className="flex h-6 w-6 items-center justify-center rounded text-text hover:bg-surface-hover"
        title={playing ? "Pause animation" : "Play animation"}
      >
        {playing ? (
          <Pause size={13} weight="fill" />
        ) : (
          <Play size={13} weight="fill" />
        )}
      </button>
      <input
        type="range"
        min={0}
        max={duration}
        step={duration / 500}
        value={Math.min(timeS, duration)}
        onChange={(e) => anim.seek(Number(e.target.value))}
        className="h-1 w-48 cursor-pointer accent-accent"
        aria-label="Animation playhead"
      />
      <span className="w-[6.5rem] whitespace-nowrap text-right font-mono text-[11px] tabular-nums text-text-muted">
        {formatTime(Math.min(timeS, duration))} / {formatTime(duration)}
      </span>
      <button
        type="button"
        onClick={() => anim.setLoop(!loop)}
        className={cn(
          "flex h-6 w-6 items-center justify-center rounded hover:bg-surface-hover",
          loop ? "text-accent" : "text-text-muted",
        )}
        title={loop ? "Looping — click for one-shot" : "One-shot — click to loop"}
      >
        <Repeat size={13} />
      </button>
      <button
        type="button"
        onClick={() => {
          const next =
            SPEEDS[(SPEEDS.indexOf(speed) + 1) % SPEEDS.length] ?? 1;
          anim.setSpeed(next);
        }}
        className="rounded px-1 font-mono text-[11px] text-text-muted hover:bg-surface-hover"
        title="Playback speed"
      >
        {speed}x
      </button>
      <button
        type="button"
        onClick={anim.hide}
        className="flex h-6 w-6 items-center justify-center rounded text-text-muted hover:bg-surface-hover"
        title="Hide animation bar"
      >
        <X size={12} />
      </button>
    </div>
  );
}
