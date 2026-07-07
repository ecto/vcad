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
import { documents, resolveDocInput } from "./session.js";
import { behavior, type ToolDef } from "./tool-def.js";

/** Most-recent report per session, used by explain / suggest / apply.
 *  Warm-only, like the print-check registries — NOT durable across a cold
 *  serverless instance or an instance flip. The inline `report` arg on
 *  explain / suggest / apply is the stateless path when this map is empty. */
const lastReports = new Map<string, DfmReport>();

/** Test/reset hook — mirrors clearPrintCheckState in print-check.ts. */
export function clearDfmState(): void {
  lastReports.clear();
}

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
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to check instead of a session. Use this stateless " +
        "path when no `document_id` is resident (e.g. a cold serverless instance). " +
        "The returned report can then be passed inline to dfm_explain / " +
        "dfm_suggest_fix / dfm_apply_fix via their `report` arg.",
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
  required: ["process"],
};

export async function dfmCheck(
  input: unknown,
  _engine: Engine,
): Promise<{ content: Array<{ type: "text"; text: string }>; isError?: boolean }> {
  const args = (input ?? {}) as Record<string, unknown>;
  const { doc, documentId: resolvedId } = resolveDocInput(args);
  // Echoed in error payloads; the empty string marks the inline path.
  const documentId = resolvedId ?? "";
  const process = String(args.process ?? "fdm");
  const rulePack = typeof args.rule_pack_toml === "string" ? args.rule_pack_toml : undefined;

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
  // Warm-cache the report only for a real session; the inline path has no id
  // to key on and hands the report straight back for inline follow-ups.
  if (documentId) lastReports.set(documentId, report);
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
    document_id: {
      type: "string" as const,
      description:
        "Session id — looks the issue up in the report dfm_check cached for it.",
    },
    issue_id: { type: "string" as const, description: "Issue id from a prior dfm_check." },
    report: {
      type: "object" as const,
      description:
        "The DFM report dfm_check returned, passed back inline. Use this when " +
        "the warm report cache is empty (cold serverless instance / instance " +
        "flip) — it resolves the issue without a resident session.",
    },
  },
  required: ["issue_id"],
};

export function dfmExplain(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const issueId = String(args.issue_id ?? "");
  const issue = findIssue(resolveReport(args, documentId), issueId);
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
    document_id: {
      type: "string" as const,
      description:
        "Session id — looks the issue up in the report dfm_check cached for it.",
    },
    issue_id: { type: "string" as const },
    report: {
      type: "object" as const,
      description:
        "The DFM report dfm_check returned, passed back inline — the stateless " +
        "path when the warm report cache is empty (cold serverless instance).",
    },
  },
  required: ["issue_id"],
};

export function dfmSuggestFix(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const issueId = String(args.issue_id ?? "");
  const issue = findIssue(resolveReport(args, documentId), issueId);
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
    document_id: {
      type: "string" as const,
      description: "Session id of the document to mutate (preferred).",
    },
    document: {
      type: "object" as const,
      description:
        "Inline Document IR to mutate instead of a session — the stateless path " +
        "when no `document_id` is resident. The mutated document is echoed back.",
    },
    issue_id: { type: "string" as const },
    report: {
      type: "object" as const,
      description:
        "The DFM report dfm_check returned, passed back inline — resolves the " +
        "issue's fix when the warm report cache is empty (cold serverless instance).",
    },
  },
  required: ["issue_id"],
};

export function dfmApplyFix(input: unknown): {
  content: Array<{ type: "text"; text: string }>;
} {
  const args = (input ?? {}) as Record<string, unknown>;
  const issueId = String(args.issue_id ?? "");
  const { doc, documentId } = resolveDocInput(args);
  const issue = findIssue(resolveReport(args, documentId ?? ""), issueId);
  if (!issue.suggested_fix) {
    throw new Error(`Issue ${issueId} has no suggested fix.`);
  }
  if (!isApplyable(issue)) {
    throw new Error(
      `Fix kind "${issue.suggested_fix.type}" not yet auto-applyable (v1 supports set_param only).`,
    );
  }
  const fix = issue.suggested_fix;
  if (fix.type === "set_param") {
    applySetParam(doc, fix.node, fix.path, fix.value);
    // Persist to the session for the id path; the inline path mutated the
    // caller's own object, which we echo back so they can retrieve it.
    if (documentId) documents.set(documentId, doc);
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
              ...(documentId ? {} : { document: doc }),
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

/** Resolve the report to look an issue up in: an inline `report` payload (the
 *  stateless path) wins; otherwise the warm cache keyed by document_id. */
function resolveReport(args: Record<string, unknown>, documentId: string): DfmReport {
  const inline = args.report;
  if (inline != null && typeof inline === "object") {
    const r = inline as DfmReport;
    if (!Array.isArray(r.issues)) {
      throw new Error(
        "`report` must be a DFM report as returned by dfm_check (with an `issues` array).",
      );
    }
    return r;
  }
  const cached = documentId ? lastReports.get(documentId) : undefined;
  if (!cached) {
    throw new Error(
      `No DFM report cached${documentId ? ` for ${documentId}` : ""}. Run dfm_check first, ` +
        "or pass the report inline via the `report` arg (survives a cold instance).",
    );
  }
  return cached;
}

function findIssue(report: DfmReport, issueId: string): DfmIssue {
  const issue = report.issues.find((i) => i.id === issueId);
  if (!issue) {
    throw new Error(`Issue ${issueId} not found in the DFM report.`);
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

export const toolDefs: ToolDef[] = [
  {
    name: "dfm_check",
    pack: "dfm",
    description:
      "Run Design-for-Manufacturing checks against an open session document. For solid parts pick a mechanical process (cnc_3axis, fdm, sla, injection, sheet_metal, casting_sand, casting_investment) and get back severities, measurements, face references, and suggested fixes. For PCB documents pick a fab profile (pcb_jlcpcb, pcb_pcbway, pcb_generic_2layer, pcb_generic_4layer) to check the board against that fab's published process capability — min annular ring, min drill, min trace/space by copper weight, copper-to-edge, soldermask dam/sliver, silk-over-pad, acid traps, and via-in-pad — returning a per-rule pass/fail report naming the profile. Each rule's threshold is sourced from a TOML pack at lib/dfm/<process>.toml — pass `rule_pack_toml` to override.",
    inputSchema: dfmCheckSchema,
    handler: (a, c) => dfmCheck(a, c.engine),
    behavior: behavior({}),
  },
  {
    name: "dfm_explain",
    pack: "dfm",
    description:
      "Return the long-form explanation for a specific DFM issue from the most recent `dfm_check` run on this document.",
    inputSchema: dfmExplainSchema,
    handler: (a) => dfmExplain(a),
    behavior: behavior({}),
  },
  {
    name: "dfm_suggest_fix",
    pack: "dfm",
    description:
      "Return the suggested patch (set_param / wrap_op / replace_op / manual) for a DFM issue. Inspect the patch; only call `dfm_apply_fix` when you're ready to mutate the IR.",
    inputSchema: dfmSuggestFixSchema,
    handler: (a) => dfmSuggestFix(a),
    behavior: behavior({}),
  },
  {
    name: "dfm_apply_fix",
    pack: "dfm",
    description:
      "Apply an approved DFM fix to the session document. v1 supports `set_param` patches (raise a fillet radius, thicken a wall) — other kinds throw and require manual edits. Re-run `dfm_check` afterwards to confirm the issue cleared.",
    inputSchema: dfmApplyFixSchema,
    handler: (a) => dfmApplyFix(a),
    behavior: behavior({ writesDoc: true, geometry: true }),
  },
];
