import { create } from "zustand";

/**
 * Long-running operation surfaced in the footer.
 *
 * Jobs exist to give the user feedback during work that would otherwise look
 * like a UI freeze (boolean ops, exports, STEP import) or to expose progress
 * + cancel for genuinely async work (network jobs, future worker-backed
 * solvers).
 */
export interface Job {
  id: string;
  /** Human-readable verb, e.g., "Importing STEP", "Exporting STL". */
  verb: string;
  /** 0..1 progress, or null for indeterminate. */
  progress: number | null;
  /** When false, the cancel button is hidden (default true). */
  cancellable: boolean;
  /** Set when the user clicked cancel. The job runner is expected to poll. */
  cancelRequested: boolean;
  startedAt: number;
}

export interface JobsState {
  jobs: Job[];
  startJob: (opts: {
    id?: string;
    verb: string;
    progress?: number | null;
    cancellable?: boolean;
  }) => string;
  updateJob: (id: string, patch: Partial<Pick<Job, "verb" | "progress">>) => void;
  finishJob: (id: string) => void;
  requestCancel: (id: string) => void;
  isCancelRequested: (id: string) => boolean;
}

let nextJobId = 1;

export const useJobsStore = create<JobsState>((set, get) => ({
  jobs: [],

  startJob: ({ id, verb, progress = null, cancellable = false }) => {
    const jobId = id ?? `job-${nextJobId++}`;
    const job: Job = {
      id: jobId,
      verb,
      progress,
      cancellable,
      cancelRequested: false,
      startedAt: performance.now(),
    };
    set((s) => ({ jobs: [...s.jobs.filter((j) => j.id !== jobId), job] }));
    return jobId;
  },

  updateJob: (id, patch) =>
    set((s) => ({
      jobs: s.jobs.map((j) => (j.id === id ? { ...j, ...patch } : j)),
    })),

  finishJob: (id) =>
    set((s) => ({ jobs: s.jobs.filter((j) => j.id !== id) })),

  requestCancel: (id) =>
    set((s) => ({
      jobs: s.jobs.map((j) => (j.id === id ? { ...j, cancelRequested: true } : j)),
    })),

  isCancelRequested: (id) => get().jobs.find((j) => j.id === id)?.cancelRequested ?? false,
}));

/**
 * Wrap a unit of work in a job. The job is registered before `fn` runs and
 * unregistered when it resolves or throws. For synchronous heavy work, an
 * rAF tick is awaited first so the chip can paint before the main thread is
 * blocked.
 */
export async function runJob<T>(
  opts: { verb: string; cancellable?: boolean; id?: string },
  fn: () => T | Promise<T>,
): Promise<T> {
  const { startJob, finishJob } = useJobsStore.getState();
  const id = startJob(opts);
  try {
    await new Promise<void>((resolve) => {
      if (typeof requestAnimationFrame !== "undefined") {
        requestAnimationFrame(() => resolve());
      } else {
        resolve();
      }
    });
    return await fn();
  } finally {
    finishJob(id);
  }
}
