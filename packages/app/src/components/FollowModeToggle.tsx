import { useCallback } from "react";
import {
  useUiStore,
  useParticipantStore,
  AI_PARTICIPANT_ID,
  LOCAL_PARTICIPANT_ID,
} from "@vcad/core";
import type { FollowMode, Participant } from "@vcad/core";
import { cn } from "@/lib/utils";

/**
 * Floating presence bar for the viewport — shows the AI participant (and
 * future peers) plus a Free / Follow / Lock toggle that controls how the
 * user's camera relates to the AI's.
 *
 *  - Free   : hide AI frustum + attention highlights; ignore AI camera.
 *  - Follow : render the AI camera as a frustum in-scene and aim the
 *             user's view at it. User stays put, but rotates to track
 *             the frustum wireframe as it moves.
 *  - Lock   : user camera matches the AI's exactly, in realtime —
 *             "see through their eyes".
 *
 * Hidden entirely when no non-local participant has joined the document
 * yet — there's nothing to follow.
 */
export function FollowModeToggle() {
  const followMode = useUiStore((s) => s.followMode);
  const setFollowMode = useUiStore((s) => s.setFollowMode);
  const setFollowingParticipant = useUiStore((s) => s.setFollowingParticipant);
  const participants = useParticipantStore((s) => s.participants);

  const apply = useCallback(
    (mode: FollowMode) => {
      setFollowMode(mode);
      setFollowingParticipant(mode === "free" ? null : AI_PARTICIPANT_ID);
    },
    [setFollowMode, setFollowingParticipant],
  );

  // Show once any non-local participant has joined, camera or not — this
  // lets the user pre-set Free before the AI's first camera move.
  const others: Participant[] = [];
  participants.forEach((p) => {
    if (p.id === LOCAL_PARTICIPANT_ID) return;
    others.push(p);
  });
  if (others.length === 0) return null;

  const btn = (mode: FollowMode, label: string, title: string) => {
    const active = followMode === mode;
    return (
      <button
        type="button"
        onClick={() => apply(mode)}
        title={title}
        className={cn(
          "px-2 py-0.5 text-[10px] font-medium rounded-sm transition-colors",
          active
            ? "bg-brand text-white"
            : "text-text-muted hover:text-text hover:bg-hover",
        )}
      >
        {label}
      </button>
    );
  };

  return (
    <div className="pointer-events-auto flex items-center gap-2 rounded-md border border-border/40 bg-surface/90 px-2 py-1 shadow-sm backdrop-blur">
      {others.map((p) => (
        <div key={p.id} className="flex items-center gap-1.5">
          <span
            aria-hidden
            className="inline-block h-2 w-2 rounded-full"
            style={{ background: p.color }}
          />
          <span className="text-[10px] font-semibold text-text">{p.name}</span>
        </div>
      ))}
      <div className="h-3 w-px bg-border/40" aria-hidden />
      <div className="flex items-center gap-0.5" role="group" aria-label="AI camera follow mode">
        {btn("free", "Free", "Free: ignore the AI camera")}
        {btn("follow", "Follow", "Follow: aim your view at the AI's camera frustum")}
        {btn("lock", "Lock", "Lock: see through the AI's camera in realtime")}
      </div>
    </div>
  );
}
