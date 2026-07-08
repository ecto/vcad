/**
 * vcad Fabricate — Phase 4 money gate (flag-gated, test-mode).
 *
 * The asymmetric-capability seam from the event-log design, applied to money:
 *   authorize_spend  — the AGENT proposes a spend authorization for a QUOTED
 *                      order (status pending_human) and emits a `propose_order`
 *                      control event on the session spine. No money moves.
 *                      When the client supports URL-mode elicitation (M3) the
 *                      approval page is carried to the human in-band — the
 *                      elicitation is an accelerator only; approval itself
 *                      stays out-of-band on vcad.io, never an MCP action.
 *   place_order      — the agent places the order ONLY after a HUMAN approved
 *                      the authorization (out of band, in the web app — never an
 *                      MCP tool). Two fail-closed gates run BEFORE the debit
 *                      (M4): the geometry gate (doc_hash must still match the
 *                      quote — the kerf intent-hash discipline applied to the
 *                      design surface) and the receipt gate (persisted
 *                      clearance claims re-verified; fail/unverifiable ⇒
 *                      refuse; no claims ⇒ proceed flagged "unverified"). On
 *                      approval it performs one atomic debit via the
 *                      debit_wallet RPC, moves the order to PAID, and emits an
 *                      `order_placed` control event.
 *
 * Ordering is OFF unless VCAD_FABRICATE_ORDERING=1 — and even then the live
 * debit path (Supabase RPC) must be staging-verified before use. Fab submission
 * (the idempotent outbox worker) is a deliberate follow-up: a placed order rests
 * at PAID for a worker to pick up.
 */

import { randomUUID } from "node:crypto";
import type { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import type { AuthUser } from "../oauth.js";
import { ownerId, type FabricateStore } from "../fabricate/store.js";
import { resolveArtifactRef } from "./artifact-store.js";
import { getSession, hydrateSession } from "./session.js";
import { docHash } from "./order.js";
import { clearanceReceiptClaims } from "./clearance.js";
import type { SessionEventStore, SessionStore } from "../session-store.js";
import type {
  FabArtifactRef,
  OrderReceiptStatus,
  SpendAuthorization,
} from "../fabricate/types.js";
import { behavior, type ToolContext, type ToolDef } from "./tool-def.js";
import { okPretty as ok, err, type ToolResult } from "./tool-result.js";

/** Ordering is disabled unless explicitly enabled — test-mode, flag-gated. */
export function orderingEnabled(): boolean {
  return process.env.VCAD_FABRICATE_ORDERING === "1";
}

const DISABLED_MSG =
  "Fabricate ordering is disabled (Phase 4, test-mode). It is flag-gated behind VCAD_FABRICATE_ORDERING and the live payment path requires staging verification before enabling. Use quote_manufacturing for estimates.";

const AUTHZ_TTL_MS = 24 * 60 * 60 * 1000; // 24h

const toUsd = (minor: number): number => Math.round(minor) / 100;

/** Human-facing dollars for elicitation copy ("12.30", never "12.3"). */
const usd = (minor: number): string => (Math.round(minor) / 100).toFixed(2);

/** Best-effort spine control event; never fails the tool (mirrors persist). */
async function emitControl(
  eventStore: SessionEventStore,
  sessionId: string,
  user: AuthUser | null,
  type: string,
  payload: Record<string, unknown>,
): Promise<void> {
  try {
    await eventStore.append(sessionId, {
      author: user?.sub ?? "agent",
      kind: "control",
      type,
      payload,
    });
  } catch {
    // best-effort — the order action already succeeded
  }
}

// ── authorize_spend ──────────────────────────────────────────────────────────

export const authorizeSpendSchema = {
  type: "object" as const,
  properties: {
    order_id: { type: "string" as const, description: "Order id from quote_manufacturing (must be QUOTED)." },
    max_amount_minor: {
      type: "integer" as const,
      minimum: 1,
      description:
        "Optional spend ceiling in minor units (USD cents). Defaults to the order total; must be ≥ the order total.",
    },
  },
  required: ["order_id"],
};

export async function authorizeSpend(
  input: unknown,
  store: FabricateStore,
  eventStore: SessionEventStore,
  user: AuthUser | null,
  elicit?: ToolContext["elicit"],
): Promise<ToolResult> {
  if (!orderingEnabled()) return err(DISABLED_MSG);

  const args = (input ?? {}) as Record<string, unknown>;
  const orderId = String(args.order_id ?? "");
  if (!orderId) return err("order_id is required.");
  const owner = ownerId(user);

  const order = await store.getOrder(orderId, owner);
  if (!order) return err(`Unknown order_id: ${orderId}`);
  if (order.state !== "QUOTED") {
    return err(`Order ${orderId} is ${order.state}, not QUOTED — nothing to authorize.`);
  }

  const max = typeof args.max_amount_minor === "number"
    ? Math.round(args.max_amount_minor)
    : order.amount_total_minor;
  if (max < order.amount_total_minor) {
    return err(
      `max_amount_minor (${max}) is below the order total (${order.amount_total_minor}).`,
    );
  }

  const now = new Date();
  const authz: SpendAuthorization = {
    id: randomUUID(),
    user_id: owner,
    quote_id: order.quote_id,
    kind: "one_time",
    max_amount_minor: max,
    daily_cap_minor: null,
    process_allowlist: null,
    fab_allowlist: order.fab ? [order.fab] : null,
    doc_hash: null,
    status: "pending_human",
    expires_at: new Date(now.getTime() + AUTHZ_TTL_MS).toISOString(),
    created_at: now.toISOString(),
  };
  await store.createAuthorization(authz, owner);
  // Link the proposal onto the order row (the order feed joins through it)
  // and append a QUOTED timeline event so the dock sees the proposal.
  await store.setOrderState(
    orderId,
    owner,
    order.state,
    `spend authorization ${authz.id} proposed (pending human approval)`,
    { authorization_id: authz.id },
  );
  await emitControl(eventStore, order.document_id, user, "propose_order", {
    order_id: orderId,
    authorization_id: authz.id,
    amount_minor: order.amount_total_minor,
    fab: order.fab,
  });

  const base = {
    authorization_id: authz.id,
    order_id: orderId,
    max_amount_usd: toUsd(max),
    expires_at: authz.expires_at,
  };
  const pendingNote =
    "Proposed. A HUMAN must approve this authorization in the vcad app before place_order can charge — the agent cannot approve its own spend. Once approved, call place_order with this authorization_id.";

  // M3: URL-mode elicitation carries the human straight to the approval page.
  // Strictly an accelerator: any failure here must never lose the created
  // authorization, so the whole exchange is fenced and falls through to the
  // out-of-band note. Approval itself always happens on vcad.io, never here.
  if (elicit?.urlSupported()) {
    try {
      const approveUrl = `https://vcad.io/authorize/${authz.id}`;
      const res = await elicit.requestUrl({
        message:
          `Approve $${usd(order.amount_total_minor)} fabrication spend for order ${orderId} ` +
          `(cap $${usd(max)}, expires in 24h)`,
        url: approveUrl,
        elicitationId: authz.id,
      });
      if (res.action === "decline") {
        // Compare-and-set: the elicitation blocks for the whole human decision
        // window, during which the human may have approved on vcad.io and a
        // concurrent place_order may have consumed the authz. A late decline
        // must never stomp 'authorized'/'consumed' → 'revoked' — re-read the
        // DB row and only revoke while the decision is still pending.
        const fresh = await store.getAuthorization(authz.id, owner);
        if (fresh && fresh.status !== "pending_human") {
          return ok({
            ...base,
            status: fresh.status,
            note:
              `Declined in chat, but the authorization is no longer pending (status: ${fresh.status}) — left untouched. ` +
              "The DB row is the truth; a stale chat decline never rewrites a decision that already happened.",
          });
        }
        await store.setAuthorizationStatus(authz.id, owner, "revoked");
        await emitControl(eventStore, order.document_id, user, "authorization_declined", {
          order_id: orderId,
          authorization_id: authz.id,
        });
        return ok({
          ...base,
          status: "revoked",
          note:
            "Declined by human — the authorization was revoked; nothing can be charged against it. Re-run authorize_spend to propose again.",
        });
      }
      if (res.action === "accept") {
        // "accept" means the human engaged the approval page — re-read the
        // truth (the DB row), never infer approval from the elicitation.
        const fresh = await store.getAuthorization(authz.id, owner);
        if (fresh?.status === "authorized") {
          return ok({
            ...base,
            status: "authorized",
            note:
              "Approved by human — ready to place. Call place_order with this authorization_id.",
          });
        }
        return ok({
          ...base,
          status: fresh?.status ?? "pending_human",
          note:
            "Approval page opened — complete the approval there, then call place_order with this authorization_id.",
        });
      }
      // "cancel" → the human dismissed the prompt; the proposal stands.
    } catch {
      // Elicitation transport failure — the authorization already exists;
      // fall through to the standard pending note.
    }
  }

  return ok({ ...base, status: authz.status, note: pendingNote });
}

// ── place_order ──────────────────────────────────────────────────────────────

export const placeOrderSchema = {
  type: "object" as const,
  properties: {
    order_id: { type: "string" as const, description: "Order id from quote_manufacturing." },
    authorization_id: {
      type: "string" as const,
      description: "Authorization id from authorize_spend (must be human-approved).",
    },
    idempotency_key: {
      type: "string" as const,
      description: "Optional. Reuse to retry safely; defaults to a per-order key.",
    },
    fab_artifact_id: {
      type: "string" as const,
      description:
        "Optional artifact id (or artifact_url) of the fab bundle from " +
        "export_gerber / export_cad. Binds the exact files to the placed order " +
        "by reference (recorded on the durable spine for the fab-submission " +
        "worker) WITHOUT re-sending them through model context. Defaults to the " +
        "artifact already bound on the order at quote time.",
    },
  },
  required: ["order_id", "authorization_id"],
};

export async function placeOrder(
  input: unknown,
  store: FabricateStore,
  eventStore: SessionEventStore,
  user: AuthUser | null,
  engine?: Engine,
  sessionStore?: SessionStore,
): Promise<ToolResult> {
  if (!orderingEnabled()) return err(DISABLED_MSG);

  const args = (input ?? {}) as Record<string, unknown>;
  const orderId = String(args.order_id ?? "");
  const authorizationId = String(args.authorization_id ?? "");
  if (!orderId || !authorizationId) {
    return err("order_id and authorization_id are required.");
  }
  const owner = ownerId(user);

  const order = await store.getOrder(orderId, owner);
  if (!order) return err(`Unknown order_id: ${orderId}`);
  if (order.state === "PAID" || order.state === "SUBMITTED" || order.state === "IN_PRODUCTION") {
    return ok({ order_id: orderId, state: order.state, note: "Order already placed." });
  }
  if (order.state !== "QUOTED") {
    return err(`Order ${orderId} is ${order.state}, not placeable.`);
  }

  const authz = await store.getAuthorization(authorizationId, owner);
  if (!authz) return err(`Unknown authorization_id: ${authorizationId}`);
  if (authz.quote_id && order.quote_id && authz.quote_id !== order.quote_id) {
    return err("Authorization is bound to a different quote than this order.");
  }
  if (authz.status === "pending_human") {
    return err(
      `Authorization ${authorizationId} is pending human approval — a human must approve it in the vcad app before this order can be placed. Do not retry until approved.`,
    );
  }
  if (authz.status === "revoked" || authz.status === "expired") {
    return err(`Authorization ${authorizationId} is ${authz.status}. Propose a new one with authorize_spend.`);
  }
  // Expiry guard for a still-authorized authz (mirrors the debit_wallet RPC).
  // Deliberately skipped for a 'consumed' authz: that path is an idempotent
  // retry finalizing an order whose debit committed but whose state write did
  // not (e.g. a crash between debit and setOrderState) — blocking it on expiry
  // would permanently strand an order the user already paid for.
  if (authz.status === "authorized" && new Date(authz.expires_at).getTime() <= Date.now()) {
    return err(`Authorization ${authorizationId} has expired. Propose a new one with authorize_spend.`);
  }
  // Both 'authorized' (first placement) and 'consumed' (idempotent retry) fall
  // through: the idempotent debit below is the single chokepoint against double
  // spend. A 'consumed' authz with no matching prior debit is rejected there
  // (authz_not_authorized), so a stale authz can't place a fresh charge.
  //
  // A 'consumed' authz means the debit ALREADY COMMITTED (crash between debit
  // and setOrderState) — this call is a replay finalizing a paid order, so the
  // pre-debit gates below are skipped: blocking the replay on post-debit drift
  // would strand a debited order at QUOTED forever with money already gone.
  const isConsumedReplay = authz.status === "consumed";

  // Durable receipt refusal: a receipt-gate failure persists receipt_status
  // "violated" on the order (see gate 2), so the refusal survives session
  // non-residency — close_document / serverless instance churn can't turn a
  // known-failing receipt into an "unverified" pass-through.
  if (!isConsumedReplay && order.receipt_status === "violated") {
    return err(
      "receipt violated — a previous place attempt found failing clearance claims on this design. " +
        "Fix the design and re-quote (a fresh quote resets the receipt) before money moves.",
    );
  }

  // Rehydrate the order's session BEFORE the gates: the dispatch layer only
  // hydrates sessions named by args.document_id, which place_order doesn't
  // carry — without this, a signed-in call (fresh per-request cache) NEVER
  // had a resident session and both gates silently skipped. Best-effort: a
  // failed hydrate degrades to the non-resident path, no worse than before.
  if (
    sessionStore &&
    order.document_id &&
    !order.document_id.startsWith("inline:")
  ) {
    try {
      await hydrateSession(sessionStore, order.document_id);
    } catch {
      // Durable load failed — the gates fall back to the non-resident note.
    }
  }

  // ── M4 gate 1: geometry must still match the quote ────────────────────────
  // The export_gerber dirty-DRC precedent applied to money: a quote is only
  // meaningful for the design it priced. When the quote carries a doc_hash and
  // the order's session is resident, re-hash and refuse on drift — the kerf
  // intent-hash discipline (geometry edit ⇒ quote dead ⇒ re-quote), enforced
  // on the design surface. Inline-IR quotes and non-resident sessions can't be
  // re-verified; they skip the gate with a note (fail-closed only when a
  // verifiable claim exists and fails).
  const quote = await store.getQuote(order.quote_id, owner);
  let sessionDoc: Document | null = null;
  let docUnavailable: string | null = null;
  if (!order.document_id || order.document_id.startsWith("inline:")) {
    docUnavailable = "quoted from inline IR — no live session to re-verify against";
  } else {
    try {
      sessionDoc = getSession(order.document_id);
    } catch {
      docUnavailable = `session ${order.document_id} is not resident on this instance`;
    }
  }
  if (!isConsumedReplay && quote?.doc_hash && sessionDoc) {
    const currentHash = docHash(sessionDoc);
    if (currentHash !== quote.doc_hash) {
      // Durable refusal: expire the order so the refusal holds even when a
      // later attempt lands on an instance where the session isn't resident
      // (EXPIRED already refuses at the non-QUOTED entry check above).
      await store.setOrderState(
        orderId,
        owner,
        "EXPIRED",
        `doc_hash mismatch — quote invalidated (quoted ${quote.doc_hash}, current ${currentHash})`,
      );
      await emitControl(eventStore, order.document_id, user, "order_blocked", {
        order_id: orderId,
        authorization_id: authorizationId,
        reason: "doc_hash_mismatch",
        quoted_doc_hash: quote.doc_hash,
        current_doc_hash: currentHash,
      });
      return err(
        `geometry changed since quote (doc_hash mismatch: quoted ${quote.doc_hash}, current ${currentHash}) — re-quote before placing.` +
          (quote.kerf_intent_hash
            ? ` The vendor quote is bound to kerf intent ${quote.kerf_intent_hash} — an edited design voids it (intent changed ⇒ quote dead).`
            : ""),
      );
    }
  }

  // ── M4 gate 2: the design receipt must hold ──────────────────────────────
  // Re-verify every persisted clearance spec at place time. Any failing claim
  // refuses; any unverifiable claim refuses too (fail-closed — a claim that
  // can't verify never passes). A document with no claims proceeds flagged
  // "unverified" in the feed; an unavailable document likewise (noted).
  let receiptStatus: OrderReceiptStatus = "unverified";
  let receiptNote: string;
  if (isConsumedReplay) {
    receiptStatus = order.receipt_status ?? "unverified";
    receiptNote =
      "consumed-authz idempotent replay — gates skipped (debit already committed); finalizing the paid order";
  } else if (!sessionDoc) {
    receiptNote = `receipt not re-verified (${docUnavailable ?? "document unavailable"}) — proceeding as unverified`;
  } else if (!sessionDoc.clearance_specs?.length) {
    receiptNote = "document carries no clearance specs — receipt status: unverified";
  } else {
    const claims = clearanceReceiptClaims(sessionDoc, engine);
    const failing = claims.filter((c) => c.verdict === "fail").map((c) => c.id);
    const unverifiable = claims
      .filter((c) => c.verdict === "unverifiable")
      .map((c) => c.id);
    if (failing.length > 0) {
      // Durable refusal: persist the verdict (state stays QUOTED) so the
      // violated-receipt entry check refuses future attempts even when the
      // session is no longer resident. Only a re-quote resets it.
      await store.setOrderState(
        orderId,
        owner,
        order.state,
        `receipt violated at place time: ${failing.join(", ")}`,
        { receipt_status: "violated" },
      );
      await emitControl(eventStore, order.document_id, user, "order_blocked", {
        order_id: orderId,
        authorization_id: authorizationId,
        reason: "receipt_violated",
        claims: failing,
      });
      return err(
        `receipt violated — clearance claims fail at place time: ${failing.join(", ")}. Fix the design (or re-quote) before money moves.`,
      );
    }
    if (unverifiable.length > 0) {
      await emitControl(eventStore, order.document_id, user, "order_blocked", {
        order_id: orderId,
        authorization_id: authorizationId,
        reason: "receipt_unverifiable",
        claims: unverifiable,
      });
      return err(
        `receipt unverifiable — clearance claims could not be re-checked: ${unverifiable.join(", ")}. Fail-closed: an unverifiable claim never passes; re-verify with check_clearance / verify_receipt before placing.`,
      );
    }
    receiptStatus = "holds";
    receiptNote = `receipt holds — ${claims.length} clearance claim(s) re-verified at place time`;
  }

  // Bind the fab bundle by reference (provided handle, else whatever the quote
  // bound). A provided-but-unresolvable handle fails BEFORE any money moves,
  // and a handle that would SWAP an already-bound bundle refuses outright: the
  // human approved a spend against the files the quote priced (the kerf
  // intent_hash pins exactly those sha256s) — substituting different files
  // after approval is a re-quote, never a place-time override.
  const fabHandle = typeof args.fab_artifact_id === "string" ? args.fab_artifact_id : "";
  let fabArtifact: FabArtifactRef | null = order.fab_artifact ?? null;
  let lateBinding = false;
  if (fabHandle) {
    const ref = resolveArtifactRef(fabHandle);
    if (!ref) {
      return err(
        `Unknown or expired fab artifact "${fabHandle}". Re-run export_gerber / export_cad and pass the artifact_id it returns.`,
      );
    }
    if (order.fab_artifact && order.fab_artifact.artifact_id !== ref.artifact_id) {
      return err(
        `fab artifact was bound at authorization time (${order.fab_artifact.artifact_id}) — ` +
          `re-quote to change files. The approved spend covers the quoted bundle; ` +
          `passing a different artifact (${ref.artifact_id}) at place time is refused before any money moves.`,
      );
    }
    fabArtifact = ref;
    lateBinding = order.fab_artifact == null;
  }
  if (lateBinding && fabArtifact) {
    // No bundle was bound at quote time — allow, but record the late binding
    // on the order's timeline so the provenance gap is visible in the feed.
    await store.setOrderState(
      orderId,
      owner,
      order.state,
      `fab artifact ${fabArtifact.artifact_id} late-bound at place time (no bundle was bound at quote time)`,
    );
  }

  const debit = await store.debit({
    userId: owner,
    amountMinor: order.amount_total_minor,
    orderId,
    authorizationId,
    idempotencyKey: typeof args.idempotency_key === "string" ? args.idempotency_key : `${orderId}:debit`,
  });
  if (!debit.ok) {
    await emitControl(eventStore, order.document_id, user, "order_payment_failed", {
      order_id: orderId,
      authorization_id: authorizationId,
      reason: debit.reason,
    });
    return err(`Payment failed: ${debit.reason ?? "unknown"}. The order remains QUOTED; resolve and retry.`);
  }

  await store.setOrderState(
    orderId,
    owner,
    "PAID",
    `place_order debit ok${debit.idempotent ? " (idempotent replay)" : ""}; receipt ${receiptStatus}`,
    { receipt_status: receiptStatus, authorization_id: authorizationId },
  );
  await emitControl(eventStore, order.document_id, user, "order_placed", {
    order_id: orderId,
    authorization_id: authorizationId,
    amount_minor: order.amount_total_minor,
    fab: order.fab,
    idempotent: debit.idempotent ?? false,
    receipt_status: receiptStatus,
    // The handle, never the bytes — the fab-submission worker fetches the files
    // from the artifact store; the manifest's sha256 verifies what it sends.
    fab_artifact: fabArtifact,
  });

  return ok({
    order_id: orderId,
    state: "PAID",
    amount_usd: toUsd(order.amount_total_minor),
    fab: order.fab,
    idempotent: debit.idempotent ?? false,
    receipt: { status: receiptStatus, note: receiptNote },
    fab_artifact: fabArtifact
      ? {
          artifact_id: fabArtifact.artifact_id,
          artifact_url: fabArtifact.artifact_url,
          bytes: fabArtifact.bytes,
          files: fabArtifact.manifest.length,
        }
      : null,
    note:
      "Paid via atomic wallet debit. Fab submission runs in a follow-up outbox slice; the order rests at PAID until then. Track with get_order_status." +
      (fabArtifact
        ? " The fab bundle is bound by reference (artifact_id); the worker fetches it from the artifact store, so the files never transit model context."
        : ""),
  });
}

export const toolDefs: ToolDef[] = [
  {
    name: "authorize_spend",
    pack: "fabricate",
    description:
      "Propose a spend authorization for a QUOTED order. Creates a DB-backed, revocable authorization (status pending_human) and records the proposal on the session's event log. A HUMAN must approve it in the vcad app before place_order can charge — the agent cannot approve its own spend; when the client supports URL elicitation the approval page is offered to the human in-band. Flag-gated (test-mode); no money moves here.",
    inputSchema: authorizeSpendSchema,
    handler: (a, c) => authorizeSpend(a, c.fabricateStore, c.eventStore, c.user, c.elicit),
    behavior: behavior({}),
  },
  {
    name: "place_order",
    pack: "fabricate",
    description:
      "Place a QUOTED order once its authorization has been human-approved: performs one atomic wallet debit and moves the order to PAID (fab submission follows in a later step). Refuses if the authorization is still pending approval, if the geometry changed since the quote (doc_hash mismatch — re-quote), or if the design's persisted clearance claims fail or cannot be re-verified (fail-closed receipt gate). Flag-gated (test-mode).",
    inputSchema: placeOrderSchema,
    handler: (a, c) =>
      placeOrder(a, c.fabricateStore, c.eventStore, c.user, c.engine, c.sessionStore),
    behavior: behavior({}),
  },
];
