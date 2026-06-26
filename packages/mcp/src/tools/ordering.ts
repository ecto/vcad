/**
 * vcad Fabricate — Phase 4 money gate (flag-gated, test-mode).
 *
 * The asymmetric-capability seam from the event-log design, applied to money:
 *   authorize_spend  — the AGENT proposes a spend authorization for a QUOTED
 *                      order (status pending_human) and emits a `propose_order`
 *                      control event on the session spine. No money moves.
 *   place_order      — the agent places the order ONLY after a HUMAN approved
 *                      the authorization (out of band, in the web app — never an
 *                      MCP tool). On approval it performs one atomic debit via
 *                      the debit_wallet RPC, moves the order to PAID, and emits
 *                      an `order_placed` control event.
 *
 * Ordering is OFF unless VCAD_FABRICATE_ORDERING=1 — and even then the live
 * debit path (Supabase RPC) must be staging-verified before use. Fab submission
 * (the idempotent outbox worker) is a deliberate follow-up: a placed order rests
 * at PAID for a worker to pick up.
 */

import { randomUUID } from "node:crypto";
import type { AuthUser } from "../oauth.js";
import { ownerId, type FabricateStore } from "../fabricate/store.js";
import { resolveArtifactRef } from "./artifact-store.js";
import type { SessionEventStore } from "../session-store.js";
import type { FabArtifactRef, SpendAuthorization } from "../fabricate/types.js";

type ToolResult = { content: Array<{ type: "text"; text: string }>; isError?: boolean };

function ok(payload: unknown): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}
function err(message: string): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify({ error: message }) }], isError: true };
}

/** Ordering is disabled unless explicitly enabled — test-mode, flag-gated. */
export function orderingEnabled(): boolean {
  return process.env.VCAD_FABRICATE_ORDERING === "1";
}

const DISABLED_MSG =
  "Fabricate ordering is disabled (Phase 4, test-mode). It is flag-gated behind VCAD_FABRICATE_ORDERING and the live payment path requires staging verification before enabling. Use quote_manufacturing for estimates.";

const AUTHZ_TTL_MS = 24 * 60 * 60 * 1000; // 24h

const toUsd = (minor: number): number => Math.round(minor) / 100;

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
  await emitControl(eventStore, order.document_id, user, "propose_order", {
    order_id: orderId,
    authorization_id: authz.id,
    amount_minor: order.amount_total_minor,
    fab: order.fab,
  });

  return ok({
    authorization_id: authz.id,
    order_id: orderId,
    status: authz.status,
    max_amount_usd: toUsd(max),
    expires_at: authz.expires_at,
    note:
      "Proposed. A HUMAN must approve this authorization in the vcad app before place_order can charge — the agent cannot approve its own spend. Once approved, call place_order with this authorization_id.",
  });
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

  // Bind the fab bundle by reference (provided handle, else whatever the quote
  // bound). A provided-but-unresolvable handle fails BEFORE any money moves.
  const fabHandle = typeof args.fab_artifact_id === "string" ? args.fab_artifact_id : "";
  let fabArtifact: FabArtifactRef | null = order.fab_artifact ?? null;
  if (fabHandle) {
    const ref = resolveArtifactRef(fabHandle);
    if (!ref) {
      return err(
        `Unknown or expired fab artifact "${fabHandle}". Re-run export_gerber / export_cad and pass the artifact_id it returns.`,
      );
    }
    fabArtifact = ref;
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

  await store.setOrderState(orderId, owner, "PAID", `place_order debit ok${debit.idempotent ? " (idempotent replay)" : ""}`);
  await emitControl(eventStore, order.document_id, user, "order_placed", {
    order_id: orderId,
    authorization_id: authorizationId,
    amount_minor: order.amount_total_minor,
    fab: order.fab,
    idempotent: debit.idempotent ?? false,
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
