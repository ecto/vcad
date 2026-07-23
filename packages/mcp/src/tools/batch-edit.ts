/**
 * `apply_edits` — atomic multi-op editing for a session document.
 *
 * Each single-node mutation (create/update/delete/set_material) is otherwise a
 * full round-trip: on the hosted server, hydrate + persist + event-append per
 * call. An agent editing a multi-feature part pays N calls for what is one
 * intent. `apply_edits` collapses that to one call: an ordered list of ops
 * applied all-or-nothing, returning one aggregated `changed` diff and one
 * integrity certificate.
 *
 * Atomicity is a swap, not a patch: ops run against a working COPY of the live
 * document, and the session is only pointed at the copy once every op has
 * succeeded. A mid-list failure therefore leaves the live session byte-identical
 * to its pre-call state — the partially-mutated copy is simply discarded — and
 * the error names the failing op index plus the underlying error. `dry_run`
 * runs the same validation against the copy but never commits it, so it reports
 * the per-op plan while mutating nothing.
 *
 * One session-scoped concern is handled by the server pipeline, not here: the
 * pre-mutation undo snapshot (so a single `undo` rewinds the whole batch) and
 * the single durable persist + event-spine append both key off this tool's
 * `writesDoc` behavior flag, exactly once per call.
 */

import type { Document } from "@vcad/ir";
import {
  documents,
  getSession,
  recordLastChanged,
  recordTriangles,
} from "./session.js";
import {
  runMutation,
  snapshotParts,
  diffParts,
  appendChanged,
} from "./registry-dispatch.js";
import { computeIntegrity, appendIntegrity } from "./integrity.js";
import { behavior, type ToolDef } from "./tool-def.js";
import type { ToolResult } from "./tool-result.js";

/** Ops a single `apply_edits` op may name — the surgical single-node mutators. */
const ALLOWED_OPS = ["create", "update", "delete", "set_material"] as const;
type AllowedOp = (typeof ALLOWED_OPS)[number];

/** Max ops per call. Beyond this an agent should split the batch — the cap
 *  keeps one call from monopolizing an instance and bounds the restore clone. */
const MAX_BATCH_OPS = 50;

export const applyEditsSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document. The batch mutates this session's document.",
    },
    ops: {
      type: "array" as const,
      description:
        "Ordered list of edit ops applied atomically (all-or-nothing). Each op is " +
        "`{op, ...args}` where op is one of create/update/delete/set_material and the " +
        "remaining fields are that tool's arguments (minus document_id): " +
        "create {op, type, params}, update {op, node_id|part_id, ...fields}, " +
        "delete {op, part_id}, set_material {op, part_id, material}. Later ops may " +
        'reference nodes created by earlier ops in the same batch symbolically: "@N" ' +
        "(in params.child/left/right/sketch or part_id/node_id) resolves to the node " +
        `created by op index N — no need to predict node ids. Max ${MAX_BATCH_OPS} per call.`,
      items: { type: "object" as const },
    },
    dry_run: {
      type: "boolean" as const,
      description:
        "When true, run planning/validation for every op against a scratch copy and " +
        "report the per-op plan WITHOUT mutating the session. Use to preflight a batch.",
    },
  },
  required: ["document_id", "ops"],
};

/** Thrown when an op in the batch fails; carries the 0-based index. */
class BatchOpError extends Error {
  constructor(
    readonly index: number,
    readonly opName: string,
    readonly cause: unknown,
  ) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    super(detail);
    this.name = "BatchOpError";
  }
}

/** Deep clone a document via the JSON round-trip openDocument uses, so the live
 *  session can never be mutated through a retained reference to the copy. */
function cloneDoc(doc: Document): Document {
  return JSON.parse(JSON.stringify(doc)) as Document;
}

/** Normalize one op entry into its tool name + forwarded args. Throws (with the
 *  index) when the op is malformed or names a disallowed tool. */
function normalizeOp(
  op: unknown,
  index: number,
): { name: AllowedOp; args: Record<string, unknown> } {
  if (!op || typeof op !== "object" || Array.isArray(op)) {
    throw new BatchOpError(index, "?", new Error("op must be an object"));
  }
  const { op: opName, ...args } = op as Record<string, unknown>;
  const name = String(opName ?? "");
  if (!ALLOWED_OPS.includes(name as AllowedOp)) {
    throw new BatchOpError(
      index,
      name || "?",
      new Error(
        `unknown op "${name}" — apply_edits ops must be one of ${ALLOWED_OPS.join(", ")}`,
      ),
    );
  }
  return { name: name as AllowedOp, args };
}

/** Matches a symbolic reference to an earlier op's created node: "@3". */
const SYMBOLIC_REF = /^@(\d+)$/;

/**
 * Resolve "@N" symbolic references in one op's args against the node ids
 * created by earlier ops in the batch, so callers never hand-predict
 * sequential node ids. Substitutes in the positions that name nodes: the
 * top-level `part_id`/`node_id` and the child refs inside `params`
 * (child/left/right/sketch/sketches). Returns a copy; throws on a forward
 * reference or a reference to an op that created no node.
 */
function resolveSymbolicRefs(
  args: Record<string, unknown>,
  createdIds: Array<string | undefined>,
  index: number,
): Record<string, unknown> {
  const resolve = (v: unknown): unknown => {
    if (typeof v !== "string") return v;
    const m = SYMBOLIC_REF.exec(v);
    if (!m) return v;
    const refIdx = Number(m[1]);
    if (refIdx >= index) {
      throw new Error(
        `"@${refIdx}" is not resolvable from op ${index} — symbolic refs may only point at EARLIER ops in the batch`,
      );
    }
    const id = createdIds[refIdx];
    if (id === undefined) {
      throw new Error(
        `"@${refIdx}" refers to op ${refIdx}, which did not create a node`,
      );
    }
    return id;
  };

  const out = { ...args };
  for (const key of ["part_id", "node_id"]) {
    if (key in out) out[key] = resolve(out[key]);
  }
  if (out.params && typeof out.params === "object" && !Array.isArray(out.params)) {
    const params = { ...(out.params as Record<string, unknown>) };
    for (const key of ["child", "left", "right", "sketch"]) {
      if (key in params) params[key] = resolve(params[key]);
    }
    if (Array.isArray(params.sketches)) {
      params.sketches = params.sketches.map(resolve);
    }
    out.params = params;
  }
  return out;
}

/**
 * Apply every op in order to `doc` (mutated in place), returning a per-op plan
 * entry for each. Throws `BatchOpError` on the first failure — the caller owns
 * rollback (it works on a throwaway copy, so a failure just discards it).
 */
function applyOps(
  doc: Document,
  ops: unknown[],
  documentId: string,
): Array<Record<string, unknown>> {
  const results: Array<Record<string, unknown>> = [];
  // Node id created by each op so far (undefined for ops that create none),
  // the targets "@N" refs resolve against.
  const createdIds: Array<string | undefined> = [];
  for (let i = 0; i < ops.length; i++) {
    const { name, args } = normalizeOp(ops[i], i);
    let result;
    try {
      result = runMutation(
        name,
        resolveSymbolicRefs(args, createdIds, i),
        doc,
        documentId,
      );
    } catch (err) {
      throw new BatchOpError(i, name, err);
    }
    // runMutation returns a single JSON text block — surface its fields as the
    // op's plan (part_id / node_id / result), tagged with the op index.
    let parsed: Record<string, unknown> = {};
    try {
      parsed = JSON.parse(result.content[0]?.text ?? "{}") as Record<string, unknown>;
    } catch {
      // non-JSON result — keep the raw text
      parsed = { result: result.content[0]?.text };
    }
    const { document_id: _docId, ...rest } = parsed;
    void _docId;
    createdIds.push(
      name === "create" && typeof rest.node_id === "string"
        ? rest.node_id
        : undefined,
    );
    results.push({ index: i, op: name, ...rest });
  }
  return results;
}

/** Handle an `apply_edits` call. */
export function applyEdits(
  args: Record<string, unknown>,
  engine?: import("@vcad/engine").Engine,
): ToolResult {
  const documentId = String(args.document_id ?? "");
  const ops = args.ops;
  const dryRun = args.dry_run === true;

  if (!Array.isArray(ops)) {
    throw new Error("apply_edits: `ops` must be an array of edit ops");
  }
  if (ops.length === 0) {
    throw new Error("apply_edits: `ops` is empty — pass at least one edit op");
  }
  if (ops.length > MAX_BATCH_OPS) {
    throw new Error(
      `apply_edits: ${ops.length} ops exceeds the ${MAX_BATCH_OPS}-op cap — split the batch into multiple calls`,
    );
  }

  const live = getSession(documentId);
  // Ops run against a copy; the live session is swapped in only on full success.
  // So a failed batch (or any dry run) never touches the session — the copy is
  // discarded and the live document stays byte-identical.
  const before = snapshotParts(live);
  const working = cloneDoc(live);

  let opResults: Array<Record<string, unknown>>;
  try {
    opResults = applyOps(working, ops, documentId);
  } catch (err) {
    if (err instanceof BatchOpError) {
      throw new Error(
        `apply_edits: op ${err.index} (${err.opName}) failed — ${err.message}. ` +
          `No changes were applied; the document is unchanged (${ops.length} ops, all rolled back).`,
      );
    }
    throw err;
  }

  if (dryRun) {
    return {
      content: [
        {
          type: "text",
          text: JSON.stringify({
            document_id: documentId,
            dry_run: true,
            planned: opResults.length,
            ops: opResults,
          }),
        },
      ],
    };
  }

  // Commit: point the session at the fully-applied copy.
  documents.set(documentId, working);

  // Narrow content shape (like the registry dispatcher) so appendChanged's
  // strict `{type:"text"}` block type is satisfied; widens to ToolResult on
  // return.
  const result: {
    content: Array<{ type: "text"; text: string }>;
    structuredContent?: Record<string, unknown>;
  } = {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: documentId,
          applied: opResults.length,
          ops: opResults,
        }),
      },
    ],
  };

  // One aggregated `changed` diff covering every op, and one integrity
  // certificate over the final document — the same trust layer every single-op
  // mutation carries, computed once for the whole batch.
  const changed = diffParts(before, snapshotParts(working));
  if (changed) {
    appendChanged(result, changed);
    recordLastChanged(documentId, [
      ...changed.added.map((p) => p.part_id),
      ...changed.modified.map((p) => p.part_id),
    ]);
    if (engine) {
      const integrity = computeIntegrity(working, engine);
      if (integrity) {
        appendIntegrity(result, integrity);
        recordTriangles(documentId, integrity.triangles);
      }
    }
  }
  return result;
}

export const toolDefs: ToolDef[] = [
  {
    name: "apply_edits",
    pack: null,
    description:
      "Apply an ordered list of edit ops (create/update/delete/set_material) to a " +
      "session document atomically — all-or-nothing. One call replaces N single-node " +
      "round-trips: it returns one aggregated `changed` diff over every op and one " +
      "integrity certificate, and a subsequent `undo` rewinds the whole batch. Any op " +
      "failing restores the pre-call document byte-for-byte and reports the failing op " +
      "index. Pass `dry_run: true` to validate and get the per-op plan without mutating. " +
      "Each op is `{op, ...args}` (e.g. {op:'create', type:'cube', params:{size:{x,y,z}}}); " +
      'later ops reference nodes created earlier in the batch as "@N" (op index N), ' +
      "so a boolean chain never predicts node ids.",
    inputSchema: applyEditsSchema,
    handler: (a, c) => applyEdits(a, c.engine),
    // mount: a batch edit is a milestone — refresh the viewer at the bottom
    // of the transcript right after a burst of changes lands.
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
