/**
 * Participant store — tracks each "user" active in the document:
 * - the local human (`local`),
 * - the AI assistant (`ai`) when a chat turn is running,
 * - future multiplayer peers (`peer:<id>`).
 *
 * Each participant owns their own camera state (position + target in
 * kernel Z-up) and their own selection. Attention rendering and the
 * Follow/Lock toggles iterate over non-local participants.
 *
 * This store is session-local. There is no persistence/transport yet —
 * when a real awareness channel lands, peer cameras will be synced
 * through it but the data shape here should generalize.
 */

import { create } from "zustand";
import type { CameraGoal } from "../camera-framing.js";

export type ParticipantKind = "local" | "ai" | "peer";

/** Canonical ID for the local human participant. */
export const LOCAL_PARTICIPANT_ID = "local";
/** Canonical ID for the chat AI participant. */
export const AI_PARTICIPANT_ID = "ai";

export interface Participant {
  id: string;
  kind: ParticipantKind;
  name: string;
  /** CSS color; used for attention tint + frustum + chip. */
  color: string;
  /**
   * Last known camera goal in kernel Z-up. `null` means this participant
   * has no camera opinion yet — the rendering layer should skip them.
   */
  camera: CameraGoal | null;
  /** Parts currently focused/selected by this participant. */
  selectedPartIds: Set<string>;
}

export interface ParticipantState {
  participants: Map<string, Participant>;

  /** Insert or replace a participant. */
  upsert: (p: Participant) => void;
  /** Remove a participant by id. No-op for the local participant. */
  remove: (id: string) => void;
  /** Update this participant's camera goal in kernel Z-up. */
  setCamera: (id: string, camera: CameraGoal | null) => void;
  /** Replace this participant's selection. */
  setSelection: (id: string, partIds: string[]) => void;
  /** Add a single part to this participant's selection. */
  addSelection: (id: string, partId: string) => void;
  /** Clear this participant's selection. */
  clearSelection: (id: string) => void;
}

function makeLocal(): Participant {
  return {
    id: LOCAL_PARTICIPANT_ID,
    kind: "local",
    name: "You",
    // The local participant's color is unused in rendering today — the
    // existing selection overlay handles local selection via its own
    // accent color. Stored here so peer code can treat all participants
    // uniformly later.
    color: "#4f9cff",
    camera: null,
    selectedPartIds: new Set(),
  };
}

/**
 * Default AI participant descriptor. Callers should upsert this (with
 * an updated name/color if they want to theme it) when a chat turn
 * begins. The store stays empty until something needs it.
 */
export function makeAiParticipant(
  overrides: Partial<Pick<Participant, "name" | "color">> = {},
): Participant {
  return {
    id: AI_PARTICIPANT_ID,
    kind: "ai",
    name: overrides.name ?? "Claude",
    color: overrides.color ?? "#c084fc",
    camera: null,
    selectedPartIds: new Set(),
  };
}

export const useParticipantStore = create<ParticipantState>((set) => ({
  participants: new Map([[LOCAL_PARTICIPANT_ID, makeLocal()]]),

  upsert: (p) =>
    set((s) => {
      const next = new Map(s.participants);
      next.set(p.id, p);
      return { participants: next };
    }),

  remove: (id) =>
    set((s) => {
      if (id === LOCAL_PARTICIPANT_ID) return s;
      if (!s.participants.has(id)) return s;
      const next = new Map(s.participants);
      next.delete(id);
      return { participants: next };
    }),

  setCamera: (id, camera) =>
    set((s) => {
      const existing = s.participants.get(id);
      if (!existing) return s;
      const next = new Map(s.participants);
      next.set(id, { ...existing, camera });
      return { participants: next };
    }),

  setSelection: (id, partIds) =>
    set((s) => {
      const existing = s.participants.get(id);
      if (!existing) return s;
      const next = new Map(s.participants);
      next.set(id, { ...existing, selectedPartIds: new Set(partIds) });
      return { participants: next };
    }),

  addSelection: (id, partId) =>
    set((s) => {
      const existing = s.participants.get(id);
      if (!existing) return s;
      const selection = new Set(existing.selectedPartIds);
      selection.add(partId);
      const next = new Map(s.participants);
      next.set(id, { ...existing, selectedPartIds: selection });
      return { participants: next };
    }),

  clearSelection: (id) =>
    set((s) => {
      const existing = s.participants.get(id);
      if (!existing) return s;
      const next = new Map(s.participants);
      next.set(id, { ...existing, selectedPartIds: new Set() });
      return { participants: next };
    }),
}));

/** Convenience: ensure the AI participant exists in the store. */
export function ensureAiParticipant(
  overrides?: Partial<Pick<Participant, "name" | "color">>,
): Participant {
  const existing = useParticipantStore.getState().participants.get(AI_PARTICIPANT_ID);
  if (existing) return existing;
  const p = makeAiParticipant(overrides);
  useParticipantStore.getState().upsert(p);
  return p;
}
