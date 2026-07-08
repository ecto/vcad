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
import { getRuntimeFlag } from "../edge-config.js";
import { behavior, type ToolDef } from "./tool-def.js";
import { okPretty as ok, err, type ToolResult } from "./tool-result.js";

/** True when the live window is enabled on this server at all. Reads the flag
 *  from Edge Config (env fallback) so a warm instance picks up a flip without a
 *  redeploy — async for that reason. */
export async function liveWindowEnabled(): Promise<boolean> {
  return getRuntimeFlag("VCAD_LIVE_WINDOW");
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
  if (!(await liveWindowEnabled())) return err(DISABLED);

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
    note: "Live link revoked — the link now 404s and no further live updates are broadcast for this session. (A viewer already connected keeps only the events they had already received; their socket isn't force-closed.)",
  });
}

export const toolDefs: ToolDef[] = [
  {
    name: "share_session",
    pack: null,
    description:
      "Share this session as a live, watchable link (mcp.vcad.io/live/<id>). Sessions are PRIVATE by default — this is the explicit opt-in that makes one viewable. Anyone with the returned link can watch the geometry + full event log (read-only) and drop annotations, so the result includes a clear public-link warning. Revoke anytime with unshare_session.",
    inputSchema: shareSessionSchema,
    handler: (a, c) => shareSession(a, c.shareStore, c.user),
    behavior: behavior({}),
  },
  {
    name: "unshare_session",
    pack: null,
    description:
      "Revoke a session's live link — it goes dead and the session is private again.",
    inputSchema: unshareSessionSchema,
    handler: (a, c) => unshareSession(a, c.shareStore),
    behavior: behavior({}),
  },
];
