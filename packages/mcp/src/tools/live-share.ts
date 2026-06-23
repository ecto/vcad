/**
 * Live review window — sharing tools.
 *
 * Sessions are PRIVATE by default. share_session is the explicit, deliberate
 * opt-in that makes a session watchable at a public link, and it says so
 * loudly. unshare_session revokes it. The /live/* HTTP routes are gated on the
 * live_shares row these tools write (see session-store.ts ShareStore +
 * migration 029), so nothing is viewable until the driver shares.
 */

import type { AuthUser } from "../oauth.js";
import type { ShareStore } from "../session-store.js";

type ToolResult = { content: Array<{ type: "text"; text: string }>; isError?: boolean };

function ok(payload: unknown): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}
function err(message: string): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify({ error: message }) }], isError: true };
}

/** True when the live window is enabled on this server at all. */
export function liveWindowEnabled(): boolean {
  return process.env.VCAD_LIVE_WINDOW === "1";
}

/** Public base URL of this MCP deployment (for building the shareable link). */
function liveBaseUrl(): string {
  const raw = process.env.VCAD_MCP_PUBLIC_URL || "https://mcp.vcad.io";
  try {
    return new URL(raw).origin;
  } catch {
    return "https://mcp.vcad.io";
  }
}

const DISABLED =
  "The live review window is not enabled on this server (set VCAD_LIVE_WINDOW=1).";

// ── share_session ────────────────────────────────────────────────────────────

export const shareSessionSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document to share as a live, watchable link.",
    },
  },
  required: ["document_id"],
};

export async function shareSession(
  input: unknown,
  store: ShareStore,
  user: AuthUser | null,
): Promise<ToolResult> {
  const documentId = String((input as { document_id?: unknown })?.document_id ?? "");
  if (!documentId) return err("document_id is required.");
  if (!liveWindowEnabled()) return err(DISABLED);

  await store.share(documentId, user?.sub ?? null);
  const link = `${liveBaseUrl()}/live/${encodeURIComponent(documentId)}`;

  return ok({
    shared: true,
    link,
    warning:
      "PUBLIC LINK — anyone who has this URL can watch this session live: see its geometry and full event log (read-only) and drop annotations. The session was PRIVATE until now; this is the moment it became shareable. Only send the link to people you want watching. Revoke anytime with unshare_session.",
    revoke_with: "unshare_session",
  });
}

// ── unshare_session ──────────────────────────────────────────────────────────

export const unshareSessionSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id to stop sharing — the live link goes dead.",
    },
  },
  required: ["document_id"],
};

export async function unshareSession(
  input: unknown,
  store: ShareStore,
): Promise<ToolResult> {
  const documentId = String((input as { document_id?: unknown })?.document_id ?? "");
  if (!documentId) return err("document_id is required.");

  await store.unshare(documentId);
  return ok({
    shared: false,
    note: "Live link revoked — the session is private again. Any open viewers will lose access on their next request.",
  });
}
