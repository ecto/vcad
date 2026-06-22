/**
 * Live review window — server-side backbone.
 *
 * The spine (session_events, migration 028) already fans every appended row out
 * over Supabase Realtime to topic `session:<id>`. This module adds the two
 * things a viewer needs that aren't kernel mutations:
 *
 *   appendOverlay — a viewer drops an annotation (pin / flag / stroke / note).
 *                   It's a `kind:'overlay'` event, so it rides the same spine
 *                   and broadcast as geometry, lands in the Receipt, and never
 *                   touches the kernel — the asymmetry is structural (overlay vs
 *                   kernel event class), not a rule.
 *   listEvents    — replay / late-join catch-up: a session's events in order,
 *                   optionally only those after a seq the client already has.
 *
 * Anchoring note: pins/flags should carry an `anchor` into the IR id space
 * (e.g. {node, face}) in their payload so they survive a re-fold; this module
 * doesn't mandate a shape yet, it just carries the payload through.
 *
 * The browser viewer app (subscribe to the topic, fold events, render the GLB,
 * draw overlays) is a separate, env-gated slice — it needs a live browser +
 * Realtime to verify, which this backbone does not.
 */

import type { SessionEventStore, StoredSessionEvent } from "../session-store.js";

/** Annotation kinds a viewer may add. Anything else is rejected. */
export const OVERLAY_TYPES = ["pin", "flag", "stroke", "note"] as const;
export type OverlayType = (typeof OVERLAY_TYPES)[number];

/** An overlay is a small annotation (a pin, a short note) — never a payload
 *  dump. Cap it so an unauthenticated annotate can't bloat the durable log or
 *  amplify over the broadcast topic. */
export const OVERLAY_PAYLOAD_MAX_BYTES = 4096;

export interface OverlayInput {
  type: string;
  payload?: unknown;
  /** Viewer-supplied display name (UNTRUSTED) — namespaced as `viewer:<name>`
   *  so it can never collide with a real user identity or 'agent'/'human'. */
  author?: string;
}

export interface OverlayOpts {
  /** A verified identity (e.g. the token email). When present it is
   *  authoritative and the untrusted `input.author` is ignored. */
  trustedAuthor?: string;
}

export type AppendOverlayResult = { ok: true } | { ok: false; error: string };

/** Resolve the spine `author` for an overlay. A verified identity wins; an
 *  anonymous viewer's self-asserted name is sanitized and `viewer:`-namespaced
 *  so it can never impersonate a real sub/email or the reserved authors. */
function resolveAuthor(input: OverlayInput, opts: OverlayOpts): string {
  if (opts.trustedAuthor) return String(opts.trustedAuthor).slice(0, 64);
  const raw = input.author ? String(input.author) : "anon";
  const safe = raw.slice(0, 48).replace(/[^\w .@-]/g, "") || "anon";
  return `viewer:${safe}`;
}

/**
 * Validate and append a viewer annotation as a `kind:'overlay'` spine event.
 * The DB trigger broadcasts it to topic `session:<id>`. Best-effort underneath
 * (eventStore.append never throws), so this resolves true once validation passes.
 */
export async function appendOverlay(
  eventStore: SessionEventStore,
  sessionId: string,
  input: OverlayInput,
  opts: OverlayOpts = {},
): Promise<AppendOverlayResult> {
  if (!sessionId) return { ok: false, error: "missing session id" };
  const type = String(input?.type ?? "");
  if (!(OVERLAY_TYPES as readonly string[]).includes(type)) {
    return { ok: false, error: `overlay type must be one of: ${OVERLAY_TYPES.join(", ")}` };
  }
  const payload =
    input.payload && typeof input.payload === "object" && !Array.isArray(input.payload)
      ? (input.payload as Record<string, unknown>)
      : {};
  let bytes: number;
  try {
    bytes = JSON.stringify(payload).length;
  } catch {
    return { ok: false, error: "overlay payload is not serializable" };
  }
  if (bytes > OVERLAY_PAYLOAD_MAX_BYTES) {
    return { ok: false, error: `overlay payload too large (${bytes} > ${OVERLAY_PAYLOAD_MAX_BYTES} bytes)` };
  }

  await eventStore.append(sessionId, {
    author: resolveAuthor(input, opts),
    kind: "overlay",
    type,
    payload,
  });
  return { ok: true };
}

/**
 * Replay / catch-up: a session's events in seq order. With `sinceSeq`, only the
 * events after it (what a live client missed while reconnecting).
 */
export async function listEvents(
  eventStore: SessionEventStore,
  sessionId: string,
  sinceSeq?: number,
): Promise<StoredSessionEvent[]> {
  if (!sessionId) return [];
  // Push the filter to the backend (server-side seq>sinceSeq), and keep the JS
  // filter as a guard so correctness holds even for a store that ignores it.
  const rows = await eventStore.list(sessionId, sinceSeq);
  return typeof sinceSeq === "number" ? rows.filter((e) => e.seq > sinceSeq) : rows;
}
