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

/** Named rulesets that layer on a process (`get_default_dfm_pack` accepts these too). */
export type DfmRuleset = "hobby-3axis-mill";

/** Pass/fail verdict for one rule — emitted by rulesets that report per rule. */
export interface DfmRuleResult {
  rule: string;
  label: string;
  passed: boolean;
  violation_count: number;
  summary: string;
  affordances: string[];
}

/** Report returned by `runDfm`. */
export interface DfmReport {
  process: DfmProcess;
  rule_pack_name: string;
  rule_pack_version: string;
  issues: DfmIssue[];
  cost_estimate: DfmCostEstimate | null;
  /** Per-rule verdicts; empty for packs that only emit issues. */
  rule_results?: DfmRuleResult[];
}

interface DfmKernelBindings {
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
  // `root_node_id` is a Rust `u64` — wasm-bindgen marshals it as a JS
  // BigInt, so callers must convert their Number-based NodeIds.
  runDfm: (process: string, rule_pack_toml: string, root_node_id: bigint) => string;
}

async function bindings(): Promise<DfmKernelBindings> {
  const wasm = (await getKernelWasm()) as unknown as Record<string, unknown>;
  return {
    get_default_dfm_pack: wasm.get_default_dfm_pack as DfmKernelBindings["get_default_dfm_pack"],
    estimate_cost_for_process: wasm.estimate_cost_for_process as DfmKernelBindings["estimate_cost_for_process"],
  };
}

export interface RunDfmOptions {
  process: DfmProcess;
  /** Optional TOML override; falls back to the bundled default. */
  rulePack?: string;
  /**
   * Named ruleset bundled at lib/dfm/<ruleset>.toml (e.g. "hobby-3axis-mill").
   * Loaded in place of the process default when `rulePack` is not given.
   */
  ruleset?: DfmRuleset | string;
}

/**
 * Run DFM checks over every part in the document. The returned report
 * flattens issues across parts; per-part grouping ships in a follow-up.
 */
export async function runDfm(
  doc: Document,
  opts: RunDfmOptions,
): Promise<DfmReport> {
  // `evaluateDocumentTS` keeps the wasm Solid handles attached to each
  // part — we route DFM through `Solid.runDfm` directly so the kernel
  // doesn't have to serialize the BRep through the WASM boundary.
  const wasm = await getKernelWasm();
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const scene = evaluateDocumentTS(doc, wasm as any);
  let pack = opts.rulePack ?? "";
  if (!pack && opts.ruleset) {
    pack = (wasm as unknown as DfmKernelBindings).get_default_dfm_pack(opts.ruleset);
  }

  const allIssues: DfmIssue[] = [];
  const ruleResults: DfmRuleResult[] = [];
  let packName = "";
  let packVersion = "1";
  // Visible roots line up with `scene.parts` (same filter as the
  // evaluator) — so the i-th part attributes its faces to the i-th
  // root NodeId. v1 coarse provenance: one NodeId per part.
  const visibleRoots = doc.roots.filter((entry) => entry.visible !== false);
  for (let i = 0; i < scene.parts.length; i++) {
    const part = scene.parts[i];
    if (!part) continue;
    const rootNodeId = visibleRoots[i]?.root ?? 0;
    const solid = (part as unknown as { solid?: DfmSolidHandle }).solid;
    if (!solid || typeof solid.runDfm !== "function") continue;
    const reportJson = solid.runDfm(opts.process, pack, BigInt(rootNodeId));
    if (!reportJson) continue;
    const report = JSON.parse(reportJson) as DfmReport;
    packName = packName || report.rule_pack_name;
    packVersion = report.rule_pack_version || packVersion;
    allIssues.push(...report.issues);
    if (report.rule_results) ruleResults.push(...report.rule_results);
  }
  return {
    process: opts.process,
    rule_pack_name: packName,
    rule_pack_version: packVersion,
    issues: allIssues,
    cost_estimate: null,
    rule_results: ruleResults,
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
 * the old rate-table hack that used to live in output-store; the
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
