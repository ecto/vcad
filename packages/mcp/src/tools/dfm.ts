/**
 * MCP tools for Design-for-Manufacturing.
 *
 * `dfm_check` runs the rule pack against an open session document and
 * returns the full report. `dfm_explain` and `dfm_suggest_fix` look up
 * a specific issue by id and return the long-form explanation / fix
 * patch. `dfm_apply_fix` mutates the session IR to apply an approved
 * patch — this is the closed-loop wedge that lets an agent paste a
 * part and get a manufacturable revision back.
 *
 * v1 ships `Manual` and `SetParam` autofix variants; `WrapOp` /
 * `ReplaceOp` require richer IR-edit primitives and land alongside
 * the FaceId→NodeId provenance refactor.
 */

import type {
  Engine,
  DfmReport,
  DfmIssue,
  DfmProcess,
  PcbFabProfile,
} from "@vcad/engine";
import { runDfm, runPcbDfm } from "@vcad/engine";
import type { Document, Node, Pcb } from "@vcad/ir";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import { documents, getSession } from "./session.js";

/** Most-recent report per session, used by explain / suggest / apply. */
const lastReports = new Map<string, DfmReport>();

const mechanicalProcessEnum = [
  "cnc_3axis",
  "fdm",
  "sla",
  "injection",
  "sheet_metal",
  "casting_sand",
  "casting_investment",
] as const;

/** PCB fab profiles, selected via `process` when the document is a board. */
const pcbProcessEnum = [
  "pcb_jlcpcb",
  "pcb_pcbway",
  "pcb_generic_2layer",
  "pcb_generic_4layer",
] as const;

const processEnum = [...mechanicalProcessEnum, ...pcbProcessEnum] as const;

/** Get the PCB from a document — PcbBoard nodes first, then legacy `doc.pcb`. */
function getDocPcb(doc: Document): Pcb | null {
  const nodeIds = getPcbNodeIds(doc);
  if (nodeIds.length > 0) return getNodePcb(doc, nodeIds[0]!);
  return (doc as Document & { pcb?: Pcb }).pcb ?? null;
}

/** Map a `process` value to a PCB fab profile, or null if it isn't a PCB one. */
function pcbProfileFor(process: string): PcbFabProfile | null {
  const norm = process.trim().toLowerCase().replace(/-/g, "_");
  const bare = norm.startsWith("pcb_") ? norm.slice(4) : norm;
  switch (bare) {
    case "jlcpcb":
    case "pcbway":
    case "generic_2layer":
    case "generic_4layer":
      return bare;
    default:
      return null;
  }
}

/** DFM score: 100 minus 30 per failed error, 10 per failed warning (floored at 0). */
function dfmScore(errors: number, warnings: number): number {
  return 100 - Math.min(100, errors * 30 + warnings * 10);
}

// ─── dfm_check ────────────────────────────────────────────────────────────

export const dfmCheckSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    process: {
      type: "string" as const,
      enum: [...processEnum],
      description:
        "Manufacturing process (mechanical part) or PCB fab profile (board) to evaluate against. " +
        "Mechanical: cnc_3axis, fdm, sla, injection, sheet_metal, casting_sand, casting_investment. " +
        "PCB fab profiles: pcb_jlcpcb, pcb_pcbway, pcb_generic_2layer, pcb_generic_4layer — these run " +
        "when the document is a PCB and check the board against that fab's published process capability " +
        "(min annular ring, drill, trace/space by copper weight, copper-to-edge, soldermask dam/sliver, " +
        "silk-over-pad, acid traps, via-in-pad). Each pack is bundled at lib/dfm/<process>.toml.",
    },
    rule_pack_toml: {
      type: "string" as const,
      description:
        "Optional TOML rule pack to override the bundled default. Same schema as lib/dfm/<process>.toml.",
    },
  },
  required: ["document_id", "process"],
};

export async function dfmCheck(
  input: unknown,
  _engine: Engine,
): Promise<{ content: Array<{ type: "text"; text: string }>; isError?: boolean }> {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const process = String(args.process ?? "fdm");
  const rulePack = typeof args.rule_pack_toml === "string" ? args.rule_pack_toml : undefined;
  const doc = getSession(documentId);

  // PCB branch: a board document checked against a fab-house capability profile.
  const profile = pcbProfileFor(process);
  if (profile) {
    const pcb = getDocPcb(doc);
    if (!pcb) {
      return {
        isError: true,
        content: [
          {
            type: "text",
            text: `Document ${documentId} has no PCB. The pcb_* profiles only apply to board documents — use a mechanical process (e.g. fdm, cnc_3axis) for solid parts.`,
          },
        ],
      };
    }
    const report = await runPcbDfm(pcb, profile, rulePack);
    if (!report) {
      return {
        isError: true,
        content: [
          { type: "text", text: "PCB DFM unavailable (kernel WASM not loaded)." },
        ],
      };
    }
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(
            {
              kind: "pcb",
              fab_profile: report.profile,
              fab_profile_name: report.profile_name,
              ...report,
              rule_count: report.rules.length,
              failed_rules: report.rules.filter((r) => !r.passed).map((r) => r.rule),
              score: dfmScore(report.error_count, report.warning_count),
            },
            null,
            2,
          ),
        },
      ],
    };
  }

  // Mechanical branch: a solid part checked against a process rule pack.
  const report = await runDfm(doc, { process: process as DfmProcess, rulePack });
  lastReports.set(documentId, report);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            ...report,
            issue_count: report.issues.length,
            score: dfmScore(
              report.issues.filter((i) => i.severity === "error").length,
              report.issues.filter((i) => i.severity === "warning").length,
            ),
          },
          null,
          2,
        ),
      },
    ],
  };
}

// ─── dfm_explain ──────────────────────────────────────────────────────────

export const dfmExplainSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const, description: "Session id." },
    issue_id: { type: "string" as const, description: "Issue id from a prior dfm_check." },
  },
  required: ["document_id", "issue_id"],
};

export function dfmExplain(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const issueId = String(args.issue_id ?? "");
  const issue = findIssue(documentId, issueId);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            id: issue.id,
            rule: issue.rule,
            severity: issue.severity,
            process: issue.process,
            message: issue.message,
            explanation: issue.explanation,
            measured: issue.measured,
            limit: issue.limit,
            units: issue.units,
          },
          null,
          2,
        ),
      },
    ],
  };
}

// ─── dfm_suggest_fix ──────────────────────────────────────────────────────

export const dfmSuggestFixSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const },
    issue_id: { type: "string" as const },
  },
  required: ["document_id", "issue_id"],
};

export function dfmSuggestFix(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const issueId = String(args.issue_id ?? "");
  const issue = findIssue(documentId, issueId);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            issue_id: issue.id,
            origin_op: issue.origin_op,
            fix: issue.suggested_fix,
            applyable: isApplyable(issue),
          },
          null,
          2,
        ),
      },
    ],
  };
}

// ─── dfm_apply_fix ────────────────────────────────────────────────────────

export const dfmApplyFixSchema = {
  type: "object" as const,
  properties: {
    document_id: { type: "string" as const },
    issue_id: { type: "string" as const },
  },
  required: ["document_id", "issue_id"],
};

export function dfmApplyFix(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const issueId = String(args.issue_id ?? "");
  const issue = findIssue(documentId, issueId);
  if (!issue.suggested_fix) {
    throw new Error(`Issue ${issueId} has no suggested fix.`);
  }
  if (!isApplyable(issue)) {
    throw new Error(
      `Fix kind "${issue.suggested_fix.type}" not yet auto-applyable (v1 supports set_param only).`,
    );
  }
  const doc = getSession(documentId);
  const fix = issue.suggested_fix;
  if (fix.type === "set_param") {
    applySetParam(doc, fix.node, fix.path, fix.value);
    documents.set(documentId, doc);
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify(
            {
              applied: true,
              node: fix.node,
              path: fix.path,
              value: fix.value,
              note: "Re-run dfm_check to confirm the issue cleared.",
            },
            null,
            2,
          ),
        },
      ],
    };
  }
  throw new Error("unreachable");
}

function findIssue(documentId: string, issueId: string): DfmIssue {
  const report = lastReports.get(documentId);
  if (!report) {
    throw new Error(
      `No DFM report cached for ${documentId}. Run dfm_check first.`,
    );
  }
  const issue = report.issues.find((i) => i.id === issueId);
  if (!issue) {
    throw new Error(`Issue ${issueId} not found in last report for ${documentId}.`);
  }
  return issue;
}

function isApplyable(issue: DfmIssue): boolean {
  return issue.suggested_fix?.type === "set_param";
}

/** Mutate `doc.nodes[node].op[path] = value`. v1 supports top-level keys
 *  (`"radius"`, `"height"`) and one nested level (`"size.x"`). */
function applySetParam(
  doc: Document,
  node: number,
  path: string,
  value: unknown,
): void {
  const target = doc.nodes[String(node)] as Node | undefined;
  if (!target) {
    throw new Error(`Node ${node} not found in document.`);
  }
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const op = target.op as any;
  const parts = path.split(".");
  let cursor = op;
  for (let i = 0; i < parts.length - 1; i++) {
    cursor = cursor[parts[i]];
    if (cursor == null) {
      throw new Error(`Path ${path} not present on op ${target.op.type}.`);
    }
  }
  cursor[parts[parts.length - 1]] = value;
}
