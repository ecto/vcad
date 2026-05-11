/**
 * Design-for-Manufacturing entry point.
 *
 * Thin TypeScript wrapper around the WASM `run_dfm_on_brep_json` and
 * `estimate_cost_for_process` bindings. The engine evaluates a Document
 * into parts, hands each part's BRep JSON to the kernel, and aggregates
 * the resulting reports.
 *
 * v1 ships the agent loop and inline annotations in the app via this
 * module; the app's QuotePanel will migrate to call `estimateCost`
 * directly so the in-app quote and the DFM cost section agree.
 */

import { getKernelWasm } from "./wasm-singleton.js";
import { evaluateDocumentTS } from "./evaluate.js";
import type { Document } from "@vcad/ir";

/** All processes the kernel-side DFM crate understands. */
export type DfmProcess =
  | "cnc_3axis"
  | "fdm"
  | "sla"
  | "injection"
  | "sheet_metal"
  | "casting_sand"
  | "casting_investment";

/** Severity tier carried by every issue. */
export type DfmSeverity = "error" | "warning" | "info";

/** Suggested fix payload — mirrors `vcad_kernel_dfm::DfmFix`. */
export type DfmFix =
  | { type: "set_param"; node: number; path: string; value: unknown }
  | { type: "wrap_op"; node: number; op_json: unknown }
  | { type: "replace_op"; node: number; op_json: unknown }
  | { type: "manual"; description: string };

/** A single manufacturability finding. */
export interface DfmIssue {
  id: string;
  rule: string;
  severity: DfmSeverity;
  process: DfmProcess;
  message: string;
  explanation: string;
  face_indices: number[];
  edge_indices: number[];
  anchor: [number, number, number];
  measured: number;
  limit: number;
  units: string;
  origin_op: number | null;
  suggested_fix: DfmFix | null;
}

/** Cost estimate emitted by `estimateCost`. */
export interface DfmCostEstimate {
  process: DfmProcess;
  material: string;
  weight_grams: number;
  material_cost_usd: number;
  machine_time_min: number | null;
  setup_cost_usd: number;
  tooling_cost_usd: number;
  total_usd: number;
  is_estimate: boolean;
  assumptions: string[];
}

/** Report returned by `runDfm`. */
export interface DfmReport {
  process: DfmProcess;
  rule_pack_name: string;
  rule_pack_version: string;
  issues: DfmIssue[];
  cost_estimate: DfmCostEstimate | null;
}

interface DfmKernelBindings {
  run_dfm_on_brep_json: (
    brep_json: string,
    process: string,
    rule_pack_toml: string,
  ) => string;
  get_default_dfm_pack: (process: string) => string;
  estimate_cost_for_process: (
    process: string,
    material_name: string,
    part_volume_mm3: number,
    stock_volume_mm3: number,
    qty: number,
    feature_count: number,
  ) => unknown;
}

interface DfmSolidHandle {
  toBrepJson: () => string | undefined;
  runDfm: (process: string, rule_pack_toml: string) => string;
}

async function bindings(): Promise<DfmKernelBindings> {
  const wasm = (await getKernelWasm()) as unknown as Record<string, unknown>;
  return {
    run_dfm_on_brep_json: wasm.run_dfm_on_brep_json as DfmKernelBindings["run_dfm_on_brep_json"],
    get_default_dfm_pack: wasm.get_default_dfm_pack as DfmKernelBindings["get_default_dfm_pack"],
    estimate_cost_for_process: wasm.estimate_cost_for_process as DfmKernelBindings["estimate_cost_for_process"],
  };
}

export interface RunDfmOptions {
  process: DfmProcess;
  /** Optional TOML override; falls back to the bundled default. */
  rulePack?: string;
}

/**
 * Run DFM checks over every part in the document. The returned report
 * flattens issues across parts; per-part grouping ships in a follow-up.
 */
export async function runDfm(
  doc: Document,
  opts: RunDfmOptions,
): Promise<DfmReport> {
  const b = await bindings();
  // `evaluateDocumentTS` keeps the wasm Solid handles attached to each
  // part — we route the BRep through `Solid.runDfm` so the kernel
  // doesn't pay for a JSON round-trip on every check.
  const wasm = await getKernelWasm();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const scene = evaluateDocumentTS(doc, wasm as any);
  const pack = opts.rulePack ?? "";

  const allIssues: DfmIssue[] = [];
  let packName = "";
  let packVersion = "1";
  for (const part of scene.parts) {
    const solid = (part as unknown as { solid?: DfmSolidHandle }).solid;
    let reportJson: string | undefined;
    if (solid && typeof solid.runDfm === "function") {
      reportJson = solid.runDfm(opts.process, pack);
    } else if (solid && typeof solid.toBrepJson === "function") {
      const brepJson = solid.toBrepJson();
      if (brepJson) {
        reportJson = b.run_dfm_on_brep_json(brepJson, opts.process, pack);
      }
    }
    if (!reportJson) continue;
    const report = JSON.parse(reportJson) as DfmReport;
    packName = packName || report.rule_pack_name;
    packVersion = report.rule_pack_version || packVersion;
    allIssues.push(...report.issues);
  }
  return {
    process: opts.process,
    rule_pack_name: packName,
    rule_pack_version: packVersion,
    issues: allIssues,
    cost_estimate: null,
  };
}

/**
 * Return the bundled default TOML rule pack for a process. Useful in
 * the app's "advanced" panel where a user can copy + tweak the pack.
 */
export async function getDefaultDfmPack(process: DfmProcess): Promise<string> {
  const b = await bindings();
  return b.get_default_dfm_pack(process);
}

export interface EstimateCostOptions {
  process: DfmProcess;
  material: string;
  partVolumeMm3: number;
  /** CNC only; defaults to `partVolumeMm3 * 2` if omitted. */
  stockVolumeMm3?: number;
  /** Molding / casting only; 0 = use rule pack default. */
  qty?: number;
  /** CNC complexity proxy: # of pockets / holes. */
  featureCount?: number;
}

/**
 * Quote-style estimate for a given process / material. This replaces
 * the old `output-store.calculatePrice` rate-table hack — the
 * QuotePanel will migrate to call this once the app integration lands.
 */
export async function estimateCost(
  opts: EstimateCostOptions,
): Promise<DfmCostEstimate> {
  const b = await bindings();
  const raw = b.estimate_cost_for_process(
    opts.process,
    opts.material,
    opts.partVolumeMm3,
    opts.stockVolumeMm3 ?? 0,
    opts.qty ?? 0,
    opts.featureCount ?? 0,
  );
  return raw as DfmCostEstimate;
}
