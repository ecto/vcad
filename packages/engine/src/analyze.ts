/**
 * Client for the Analyze worker (#592): runs structural FEA and tolerance
 * stackup studies off the main thread and returns typed results, including
 * the receipt claims every result must ship with (fail-closed).
 */

import type { ReceiptClaim } from "@vcad/ir";

/** One FEA refinement level's scalar summary (kernel `Solution`). */
export interface FeaSolutionSummary {
  max_displacement_mm: number;
  max_displacement_at: [number, number, number];
  max_von_mises_mpa: number;
  max_stress_at: [number, number, number];
  compliance_n_mm: number;
  volume_mm3: number;
  nodes: number;
  tets: number;
  h_mm: number;
  grid: [number, number, number];
  iterations: number;
  residual_rel: number;
}

/** The convergence-gated FEA study (kernel `ConvergedAnalysis`). */
export interface FeaStudyResult {
  levels: FeaSolutionSummary[];
  displacement_change_rel: number;
  stress_change_rel: number;
  verdict: { verdict: "Converged" } | { verdict: "Unverifiable"; reasons: string[] };
  safety_factor: number | null;
  /**
   * Measured wall thickness against the lattice pitch (kernel
   * `ThinWallDiagnosis`). `blocking_advice` is set when the pitch cannot
   * resolve the thinnest load-bearing section — the verdict is then
   * Unverifiable regardless of what the QoIs did between levels, and the
   * advice names the closed-form route (`beam_check`).
   */
  thin_wall: {
    thickness: {
      min_mm: number;
      p05_mm: number;
      median_mm: number;
      thin_axis: string;
      samples: number;
      longest_bbox_mm: number;
    };
    finest_pitch_mm: number;
    cells_through_section: number;
    required_resolution: number;
    resolution_cap: number;
    reachable: boolean;
    blocking_advice: string | null;
    advisory: string | null;
  };
}

/** Full `feaAnalyzeMesh` payload. */
export interface FeaAnalysis {
  study: FeaStudyResult;
  claim_set: unknown | null;
  receipt_claims: ReceiptClaim[];
  /** Per-surface-vertex displacement magnitude, mm (when fields requested). */
  vertex_displacement_mm?: number[];
  /** Per-surface-vertex von Mises stress, MPa (when fields requested). */
  vertex_von_mises_mpa?: number[];
}

/** One tolerance distribution summary. */
export interface ToleranceDistribution {
  nominal_mm?: number;
  mean_mm?: number;
  sigma_mm?: number;
  min_mm?: number;
  max_mm?: number;
  yield_fraction?: number;
  [key: string]: unknown;
}

/** Full `toleranceAnalyze` payload. */
export interface ToleranceAnalysis {
  worst_case: ToleranceDistribution;
  rss: ToleranceDistribution;
  monte_carlo: ToleranceDistribution;
  sensitivities: Array<{ name: string; [key: string]: unknown }>;
  claim_set: unknown;
  receipt_claims: ReceiptClaim[];
}

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

/**
 * Lazily-created singleton worker running the solver fleet. Each request
 * gets a unique id; superseded studies simply resolve later and are
 * ignored by the store (it keys results by study id + revision).
 */
export class AnalyzeClient {
  private worker: Worker | null = null;
  private ready: Promise<void> | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();

  private ensureWorker(): Promise<void> {
    if (this.ready) return this.ready;
    if (typeof Worker === "undefined") {
      return Promise.reject(new Error("Web Workers unavailable"));
    }
    const worker = new Worker(new URL("./analyze-worker.js", import.meta.url), {
      type: "module",
    });
    this.worker = worker;
    this.ready = new Promise<void>((resolve, reject) => {
      const onMessage = (e: MessageEvent) => {
        const { type, id, message, analysis } = e.data;
        if (type === "ready") {
          resolve();
          return;
        }
        if (type === "error" && id === null) {
          reject(new Error(message));
          this.worker = null;
          this.ready = null;
          return;
        }
        const p = this.pending.get(id);
        if (!p) return;
        this.pending.delete(id);
        if (type === "result") p.resolve(analysis);
        else p.reject(new Error(message));
      };
      worker.addEventListener("message", onMessage);
      worker.postMessage({ type: "init" });
    });
    return this.ready;
  }

  private request<T>(msg: Record<string, unknown>, transfer: Transferable[] = []): Promise<T> {
    return this.ensureWorker().then(
      () =>
        new Promise<T>((resolve, reject) => {
          const id = this.nextId++;
          this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
          this.worker!.postMessage({ ...msg, id }, transfer);
        }),
    );
  }

  /** Run a structural FEA study on a surface mesh (copies the arrays). */
  runStructural(
    spec: unknown,
    options: unknown,
    positions: Float32Array,
    indices: Uint32Array,
  ): Promise<FeaAnalysis> {
    // Copy so the caller's live viewport mesh buffers are not detached.
    const p = positions.slice();
    const i = indices.slice();
    return this.request<FeaAnalysis>(
      {
        type: "fea",
        specJson: JSON.stringify(spec),
        optionsJson: JSON.stringify(options ?? {}),
        positions: p,
        indices: i,
      },
      [p.buffer, i.buffer],
    );
  }

  /** Run a tolerance stackup study. */
  runTolerance(
    spec: unknown,
    params?: Record<string, number>,
    options?: unknown,
  ): Promise<ToleranceAnalysis> {
    return this.request<ToleranceAnalysis>({
      type: "tolerance",
      specJson: JSON.stringify(spec),
      paramsJson: JSON.stringify(params ?? {}),
      optionsJson: JSON.stringify(options ?? {}),
    });
  }

  /** Terminate the worker (e.g. on document close). */
  dispose(): void {
    this.worker?.terminate();
    this.worker = null;
    this.ready = null;
    for (const p of this.pending.values()) p.reject(new Error("Analyze worker disposed"));
    this.pending.clear();
  }
}

let _client: AnalyzeClient | null = null;

/** The shared AnalyzeClient instance. */
export function getAnalyzeClient(): AnalyzeClient {
  if (!_client) _client = new AnalyzeClient();
  return _client;
}
