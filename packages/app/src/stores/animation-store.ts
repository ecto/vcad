/**
 * Animation playback store — transport state for the document timeline
 * (`doc.timeline`, authored via the MCP `animate` tool or the kernel).
 *
 * Playback never mutates the document: the playback hook samples joint
 * tracks, runs FK on a temp clone, and pushes transient instance transforms
 * through the engine store (same pattern as the physics loop).
 */

import { create } from "zustand";

export interface AnimationState {
  /** Transport bar visible (auto-opens when a doc with a timeline loads). */
  visible: boolean;
  /** Currently advancing time. */
  playing: boolean;
  /** Playhead in seconds. */
  timeS: number;
  /** Loop at the end (default on — cycles read as running machines). */
  loop: boolean;
  /** Playback speed multiplier. */
  speed: number;
  /** Bumped by the playback hook when it applies a pose; UI-only. */

  show: () => void;
  hide: () => void;
  play: () => void;
  pause: () => void;
  togglePlay: () => void;
  /** Scrub to a time (clamped by the hook against the doc's duration). */
  seek: (timeS: number) => void;
  setLoop: (loop: boolean) => void;
  setSpeed: (speed: number) => void;
}

export const useAnimationStore = create<AnimationState>((set) => ({
  visible: false,
  playing: false,
  timeS: 0,
  loop: true,
  speed: 1,

  show: () => set({ visible: true }),
  hide: () => set({ visible: false, playing: false }),
  play: () => set({ playing: true, visible: true }),
  pause: () => set({ playing: false }),
  togglePlay: () => set((s) => ({ playing: !s.playing, visible: true })),
  seek: (timeS) => set({ timeS, playing: false }),
  setLoop: (loop) => set({ loop }),
  setSpeed: (speed) => set({ speed }),
}));
