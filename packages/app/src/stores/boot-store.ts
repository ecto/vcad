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

  setPhase: (p: BootPhase) => void;
  setFetchProgress: (received: number, total: number) => void;
  setSlowNetwork: (s: boolean) => void;
  setError: (e: string) => void;
}

export const useBootStore = create<BootState>((set) => ({
  phase: "fetching-kernel",
  bytesReceived: 0,
  bytesTotal: 0,
  slowNetwork: false,
  error: null,

  setPhase: (phase) => set({ phase }),
  setFetchProgress: (bytesReceived, bytesTotal) =>
    set({ bytesReceived, bytesTotal }),
  setSlowNetwork: (slowNetwork) => set({ slowNetwork }),
  setError: (error) => set({ error }),
}));
