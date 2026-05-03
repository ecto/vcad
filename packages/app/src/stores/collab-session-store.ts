import { create } from "zustand";

/**
 * Shared collab-session metadata produced by `useCollabSync` and consumed
 * by other realtime features (XR presence, future cursor sync).
 *
 * `cloudId` is the resolved Supabase document id, or null when the doc
 * isn't cloud-synced yet. It is null while signed-out, in read-only share
 * mode, or before the boot phase is ready.
 */
interface CollabSessionState {
  cloudId: string | null;
  setCloudId: (id: string | null) => void;
}

export const useCollabSessionStore = create<CollabSessionState>((set) => ({
  cloudId: null,
  setCloudId: (cloudId) => set({ cloudId }),
}));
