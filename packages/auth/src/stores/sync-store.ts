import { create } from "zustand";

export type SyncStatus = "idle" | "syncing" | "synced" | "error";

/** A conflict record created when local edits clash with a newer cloud version. */
export interface SyncConflict {
  /** The original document ID whose local changes conflicted. */
  originalId: string;
  /** The forked document ID that preserved the local changes. */
  forkId: string;
  /** Display name of the fork document. */
  forkName: string;
  /** Unix timestamp when the conflict was detected. */
  detectedAt: number;
}

interface SyncState {
  /** Current sync status */
  syncStatus: SyncStatus;
  /** Unix timestamp of last successful sync */
  lastSyncAt: number | null;
  /** Number of documents pending upload */
  pendingCount: number;
  /** Last sync error message */
  error: string | null;
  /** Conflicts detected since last acknowledgement */
  conflicts: SyncConflict[];

  // Actions
  setSyncStatus: (status: SyncStatus) => void;
  setLastSyncAt: (time: number) => void;
  setPendingCount: (count: number) => void;
  setError: (error: string | null) => void;
  addConflict: (conflict: SyncConflict) => void;
  clearConflicts: () => void;
  reset: () => void;
}

export const useSyncStore = create<SyncState>((set) => ({
  syncStatus: "idle",
  lastSyncAt: null,
  pendingCount: 0,
  error: null,
  conflicts: [],

  setSyncStatus: (syncStatus) => set({ syncStatus }),
  setLastSyncAt: (lastSyncAt) => set({ lastSyncAt }),
  setPendingCount: (pendingCount) => set({ pendingCount }),
  setError: (error) => set({ error }),
  addConflict: (conflict) =>
    set((state) => ({ conflicts: [...state.conflicts, conflict] })),
  clearConflicts: () => set({ conflicts: [] }),
  reset: () =>
    set({
      syncStatus: "idle",
      lastSyncAt: null,
      pendingCount: 0,
      error: null,
      conflicts: [],
    }),
}));
