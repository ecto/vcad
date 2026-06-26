/**
 * checkpoint_document / branch_from — durable snapshots of a session's
 * known-good state.
 *
 * WHY: an MCP design session is expensive to rebuild — declare a netlist, place
 * components, route nets, run DRC. The netlist especially is the most expensive
 * and most stable artifact. When a route attempt goes wrong, or the agent wants
 * to explore a variant, rebuilding from scratch wastes turns and reintroduces
 * mistakes. `checkpoint_document` snapshots the full Document IR (netlist
 * included) under an unguessable id; `branch_from` re-opens that snapshot as a
 * fresh session — or restores it into an existing one — so the agent rewinds to
 * a good state instead of rebuilding.
 *
 * A checkpoint is just ANOTHER durable document: it uses the same SessionStore
 * (the user-owned `documents` table, or the capability-keyed `mcp_sessions`
 * table) as the live session, so there is no new table and a checkpoint inherits
 * the exact cold-instance survival the live session has. On a durable deploy a
 * checkpoint outlives a redeploy; on stdio/local it lives for the process like
 * any other session (the in-memory store's save is a no-op, but the snapshot is
 * warm-cached in the `documents` map).
 */

import { randomBytes } from "node:crypto";
import type { Document } from "@vcad/ir";
import type { SessionStore } from "../session-store.js";
import { documents, getSession, registerSession } from "./session.js";

type ToolText = {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
};

function err(text: string): ToolText {
  return { isError: true, content: [{ type: "text", text }] };
}

function ok(body: Record<string, unknown>): ToolText {
  return { content: [{ type: "text", text: JSON.stringify(body) }] };
}

let nextCheckpoint = 1;

/** Unguessable checkpoint id. The counter keeps intra-process uniqueness; the
 *  random suffix makes the id a capability (the anonymous store keys rows by it
 *  alone), so one caller can't enumerate another's checkpoints. */
function nextCheckpointId(): string {
  return `ckpt_${nextCheckpoint++}_${randomBytes(9).toString("base64url")}`;
}

/** A light, non-throwing summary of what the snapshot captures — surfaced so the
 *  agent can see the netlist anchor (and placement/routing progress) is held,
 *  without re-fetching the whole IR. Reads both `pcb` (post-place/route) and
 *  `schematic` (the netlist declared as data before place_components). */
function snapshotSummary(doc: Document): Record<string, number> {
  const s: Record<string, number> = { parts: doc.roots?.length ?? 0 };
  const pcb = (
    doc as {
      pcb?: {
        nets?: unknown[];
        footprints?: unknown[];
        traces?: unknown[];
        vias?: unknown[];
      };
    }
  ).pcb;
  if (pcb) {
    if (Array.isArray(pcb.nets)) s.nets = pcb.nets.length;
    if (Array.isArray(pcb.footprints)) s.components = pcb.footprints.length;
    if (Array.isArray(pcb.traces)) s.traces = pcb.traces.length;
    if (Array.isArray(pcb.vias)) s.vias = pcb.vias.length;
  }
  const sch = (
    doc as {
      schematic?: { components?: unknown[]; nets?: Record<string, unknown> };
    }
  ).schematic;
  if (sch) {
    if (Array.isArray(sch.components)) {
      s.schematic_components = sch.components.length;
    }
    if (sch.nets && typeof sch.nets === "object") {
      s.schematic_nets = Object.keys(sch.nets).length;
    }
  }
  return s;
}

// ─── checkpoint_document ──────────────────────────────────────────────────

export const checkpointDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "Session id (from open_document / create_schematic / load_document) to snapshot.",
    },
    label: {
      type: "string" as const,
      description:
        "Optional human label for the checkpoint, e.g. 'post-schematic', " +
        "'post-place', or 'post-route'. Echoed back by branch_from.",
    },
  },
  required: ["document_id"],
};

/**
 * Snapshot a session's current Document IR as a durable, restorable checkpoint.
 * The dispatch layer has already hydrated `document_id` from the durable store,
 * so this works on a cold instance. The snapshot is deep-copied so later edits
 * to the live session never mutate it.
 */
export async function checkpointDocument(
  args: Record<string, unknown>,
  store: SessionStore,
): Promise<ToolText> {
  const sourceId = String(args.document_id ?? "");
  const label =
    typeof args.label === "string" && args.label.trim()
      ? args.label.trim()
      : undefined;

  // getSession throws the pinned "Unknown document_id" error if the session
  // isn't resident (and couldn't be hydrated) — same contract as every reader.
  const doc = getSession(sourceId);

  // Deep copy: the checkpoint must be frozen at this moment, independent of any
  // further edits to the live session.
  const snapshot = JSON.parse(JSON.stringify(doc)) as Document;
  const checkpointId = nextCheckpointId();

  // Warm-cache the snapshot so a branch on this same instance is instant, and
  // so stdio/local (where store.save is a no-op) still resolves it. On a durable
  // deploy store.save persists it so the checkpoint outlives a redeploy.
  documents.set(checkpointId, snapshot);
  await store.save(checkpointId, snapshot, label ?? "checkpoint");

  return ok({
    checkpoint_id: checkpointId,
    of: sourceId,
    label: label ?? null,
    summary: snapshotSummary(snapshot),
    hint:
      "Saved a restorable snapshot — the netlist (the most expensive artifact) " +
      "is captured. Fork it later with branch_from, or restore it in place with " +
      "branch_from(into: <document_id>).",
  });
}

// ─── branch_from ──────────────────────────────────────────────────────────

export const branchFromSchema = {
  type: "object" as const,
  properties: {
    checkpoint_id: {
      type: "string" as const,
      description: "A checkpoint id returned by checkpoint_document.",
    },
    into: {
      type: "string" as const,
      description:
        "Optional. An existing session id to RESTORE the checkpoint into — " +
        "overwrites that session's content but keeps its id, so existing " +
        "handles keep working. Omit to BRANCH into a fresh session id instead.",
    },
  },
  required: ["checkpoint_id"],
};

/**
 * Re-open a checkpoint. With no `into`, branches into a fresh session id (a
 * variant to explore); with `into`, restores the checkpoint into that existing
 * session in place. Resolves the checkpoint from the warm cache first, then the
 * durable store — so it survives a redeploy. The result carries `document_id`,
 * which the dispatch persist wrapper writes back to the durable store (branch_from
 * is a doc-writer), so the branch/restore is itself durable.
 */
export async function branchFrom(
  args: Record<string, unknown>,
  store: SessionStore,
): Promise<ToolText> {
  const checkpointId = String(args.checkpoint_id ?? "");
  if (!checkpointId) {
    return err(
      "branch_from requires a `checkpoint_id` from a prior checkpoint_document call.",
    );
  }

  // Warm cache first, then the durable store: a cold instance after a redeploy
  // has an empty cache but the checkpoint row survives.
  let snapshot = documents.get(checkpointId) ?? null;
  if (!snapshot) snapshot = await store.load(checkpointId);
  if (!snapshot) {
    return err(
      `Unknown checkpoint_id "${checkpointId}". Create one with ` +
        `checkpoint_document first. (If it existed earlier, this deploy may be ` +
        `running without a durable session store — check server_info for ` +
        `durable:false.)`,
    );
  }

  // Defensive copy — the branch/restore is an independent document; editing it
  // must not mutate the checkpoint, so the same checkpoint can be reused.
  const copy = JSON.parse(JSON.stringify(snapshot)) as Document;

  const into =
    typeof args.into === "string" && args.into.trim() ? args.into.trim() : null;
  if (into) {
    // Restore in place: overwrite the existing session, keep its id.
    documents.set(into, copy);
    return ok({
      document_id: into,
      restored_from: checkpointId,
      summary: snapshotSummary(copy),
      hint:
        "Restored in place — the session id is unchanged. Re-render with render_view.",
    });
  }

  // Branch: a fresh session id. The dispatch persist wrapper writes it through
  // to the durable store so the fork survives a redeploy too.
  const newId = registerSession(copy);
  return ok({
    document_id: newId,
    branched_from: checkpointId,
    summary: snapshotSummary(copy),
    hint:
      "Forked into a new session. Continue here, or branch_from the same " +
      "checkpoint again to try another variant.",
  });
}
