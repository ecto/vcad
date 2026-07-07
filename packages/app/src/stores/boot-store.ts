import { create } from "zustand";

export type BootPhase =
  | "fetching-kernel"
  | "starting-engine"
  | "loading-document"
  | "evaluating"
  | "ready";

export interface BootState {
  phase: BootPhase;
  bytesReceived: number;
  bytesTotal: number;
  slowNetwork: boolean;
  error: string | null;
  /** True when boot found no existing document anywhere (URL, route,
   * last-opened slot, IDB) — a fresh profile. Drives the first-run
   * template gallery. */
  firstVisit: boolean;

  setPhase: (p: BootPhase) => void;
  setFetchProgress: (received: number, total: number) => void;
  setSlowNetwork: (s: boolean) => void;
  setError: (e: string) => void;
  setFirstVisit: (v: boolean) => void;
}

export const useBootStore = create<BootState>((set) => ({
  phase: "fetching-kernel",
  bytesReceived: 0,
  bytesTotal: 0,
  slowNetwork: false,
  error: null,
  firstVisit: false,

  setPhase: (phase) => set({ phase }),
  setFetchProgress: (bytesReceived, bytesTotal) =>
    set({ bytesReceived, bytesTotal }),
  setSlowNetwork: (slowNetwork) => set({ slowNetwork }),
  setError: (error) => set({ error }),
  setFirstVisit: (firstVisit) => set({ firstVisit }),
}));
