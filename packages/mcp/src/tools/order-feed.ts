/**
 * get_order_feed — app-only feed behind the viewer's order dock (M2).
 *
 * The single read the mounted canvas polls to render the fused vcad+kerf
 * order lifecycle: per-order state chips, the joined quote's process/qty/
 * lead/pricing-basis, authorization status (+ the approve URL while a human
 * decision is pending), receipt status, and the wallet balance footer.
 *
 * Security posture: the iframe is READ-ONLY for money. This is the only
 * ordering-adjacent tool the widget can call, and it only reads — the
 * asymmetric seam (agent proposes, human approves out-of-band, agent places)
 * stays intact. Margin invariant preserved: totals only, never fab internals.
 */

import type { AuthUser } from "../oauth.js";
import { ownerId, type FabricateStore } from "../fabricate/store.js";
import { orderStateChip } from "../fabricate/state-chip.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { ok, err, type ToolResult } from "./tool-result.js";

const toUsd = (minor: number): number => Math.round(minor) / 100;

/** FNV-1a change token over the feed's identity (order ids + states + event
 *  counts) — the dock polls this-shaped payloads and re-renders on change
 *  (pattern: preview.ts's previewVersion). */
function feedVersion(parts: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < parts.length; i++) {
    h ^= parts.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

export const getOrderFeedSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session/document id whose orders to feed.",
    },
  },
  required: ["document_id"],
};

export async function getOrderFeed(
  input: unknown,
  store: FabricateStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  if (!documentId) return err("document_id is required.");
  const owner = ownerId(user);

  // Owner-scoped, newest first; quote-only orders (state QUOTED, no money
  // moved) belong in the dock too — the filter is by document, not by state.
  // The store limit applies BEFORE the document filter, so fetch a wide
  // window (200) and slice to the dock's 20 AFTER filtering — otherwise 20
  // newer orders on other documents push this document's live approvals out
  // of the feed entirely.
  const all = await store.listOrders(owner, { limit: 200 });
  const mine = all.filter((o) => o.document_id === documentId).slice(0, 20);

  const orders: Array<Record<string, unknown>> = [];
  const versionParts: string[] = [];
  for (const o of mine) {
    const quote = o.quote_id ? await store.getQuote(o.quote_id, owner) : null;
    const authz = o.authorization_id
      ? await store.getAuthorization(o.authorization_id, owner)
      : null;
    // The recommended option leads the sorted fab_options; prefer the one the
    // order actually routed to.
    const option =
      quote?.fab_options.find((f) => f.fab === o.fab) ?? quote?.fab_options[0] ?? null;

    orders.push({
      order_id: o.order_id,
      state_chip: orderStateChip(o.state, authz?.status),
      raw_state: o.state,
      ...(quote ? { process: quote.process, quantity: quote.quantity } : {}),
      total_amount_usd: toUsd(o.amount_total_minor),
      ...(option ? { pricing_basis: option.pricing_basis } : {}),
      vendor: o.fab,
      ...(option ? { lead_time_days: option.lead_time_days } : {}),
      ...(quote?.expires_at ? { quote_expires_at: quote.expires_at } : {}),
      created_at: o.created_at,
      events: o.events,
      authorization: authz
        ? {
            status: authz.status,
            max_amount_usd: toUsd(authz.max_amount_minor),
            expires_at: authz.expires_at,
            // The dock's approve button leaves the iframe — the widget never
            // approves; only surface the URL while a human decision is open.
            ...(authz.status === "pending_human"
              ? { approve_url: `https://vcad.io/authorize/${authz.id}` }
              : {}),
          }
        : null,
      tracking: null,
      receipt: { status: o.receipt_status ?? "unverified" },
      ...(o.kerf_intent_hash ? { kerf_intent_hash: o.kerf_intent_hash } : {}),
    });
    // Version input covers everything the rendered card is derived from:
    // authorization status (human approval flips ONLY the authz row — no
    // order state change, no new event), receipt verdict, and pricing basis,
    // so the dock re-renders the moment any of them move.
    versionParts.push(
      `${o.order_id}:${o.state}:${o.events.length}:${authz?.status ?? "-"}:${o.receipt_status ?? "-"}:${option?.pricing_basis ?? "-"}`,
    );
  }

  const walletMinor = await store.getWalletBalance(owner);
  // Wallet balance is part of the identity too — a top-up or debit must
  // refresh the footer even when no order changed.
  const version = feedVersion(
    `${versionParts.join("|")}|w:${walletMinor ?? "-"}`,
  );

  return ok({
    orders,
    wallet_balance_usd: walletMinor == null ? null : toUsd(walletMinor),
    version,
  });
}

export const toolDefs: ToolDef[] = [
  {
    name: "get_order_feed",
    pack: "fabricate",
    description:
      "App-only: the order-dock feed for a document — orders with fused lifecycle chips, joined quote details, authorization status (+ approve URL while pending), receipt status, and wallet balance. Polled by the inline viewer; read-only.",
    inputSchema: getOrderFeedSchema,
    handler: (a, c) => getOrderFeed(a, c.fabricateStore, c.user),
    behavior: behavior({ appOnly: true }),
  },
];
