import { create } from "zustand";
import type { XRPresenceUpdate } from "@vcad/auth";

/** Time after which a peer is considered stale and dropped from the map. */
const STALE_MS = 5_000;

/**
 * Remote XR participant tracking — populated by inbound broadcasts on the
 * XR collab channel. Poses are stored in scene-local coordinates so they
 * can be rendered inside the same `XRSceneTransform` group regardless of
 * each user's physical room frame.
 */
interface XRPresenceState {
  peers: Map<string, XRPresenceUpdate>;
  /** Ingest a pose update from the wire. */
  ingest: (update: XRPresenceUpdate) => void;
  /** Drop a peer (remote user explicitly left). */
  drop: (userId: string) => void;
  /** Sweep stale peers based on `ts`. Call from a slow interval. */
  pruneStale: () => void;
  /** Drop everything (e.g. on session end). */
  clear: () => void;
}

export const useXRPresenceStore = create<XRPresenceState>((set, get) => ({
  peers: new Map(),
  ingest: (update) =>
    set((s) => {
      const next = new Map(s.peers);
      next.set(update.userId, update);
      return { peers: next };
    }),
  drop: (userId) =>
    set((s) => {
      if (!s.peers.has(userId)) return s;
      const next = new Map(s.peers);
      next.delete(userId);
      return { peers: next };
    }),
  pruneStale: () => {
    const now = Date.now();
    const peers = get().peers;
    let removed = false;
    const next = new Map(peers);
    for (const [id, p] of peers) {
      if (now - p.ts > STALE_MS) {
        next.delete(id);
        removed = true;
      }
    }
    if (removed) set({ peers: next });
  },
  clear: () => set({ peers: new Map() }),
}));
