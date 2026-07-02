/**
 * PostHog telemetry for MCP tool usage.
 *
 * Emits one `mcp_tool_call` event per tool invocation so we can see which tools
 * agents actually use, error rates, and signed-in vs anonymous traffic — the
 * event-level companion to notify.ts's Discord rollups, fed from the same single
 * chokepoint (`fireToolAlert`). Disabled until POSTHOG_API_KEY is set, so local
 * stdio installs, dev, and the test suite stay silent and offline.
 *
 * Design constraints (mirroring notify.ts):
 *   - Never block the tool response. capture() starts a fetch and returns.
 *   - Never throw into the caller. Every failure is swallowed to stderr.
 *   - Don't leak payloads. Only the tool name, error flag, session id, signed-in
 *     flag, and build identity are sent — never argument values or IR.
 *
 * Serverless delivery: a Vercel function can freeze the instant it returns,
 * killing an in-flight fetch. So capture() registers its promise in `pending`
 * and the transport entry awaits flushTelemetry() after handling each request.
 */

import { currentUser } from "./tools/session.js";

/**
 * Notifier configuration. The PostHog project key (the public `phc_…` write
 * key) and host come from the environment, so they're set on the hosted server
 * (Vercel) without enabling telemetry for every local install. Empty key =
 * disabled. Tests/dev override these fields directly.
 */
export const telemetryConfig = {
  /** PostHog project API key (public `phc_…` write key). Empty = disabled. */
  apiKey: process.env.POSTHOG_API_KEY || "",
  /** PostHog ingestion host. */
  host: process.env.POSTHOG_HOST || "https://us.i.posthog.com",
};

/** Static build identity, set once by the server module at load. */
let buildContext: Record<string, string> = {};

/** Called once from server.ts so events carry the running version/commit. */
export function configureTelemetry(info: {
  version: string;
  build_sha: string;
  instance_id: string;
}): void {
  buildContext = {
    mcp_version: info.version,
    build_sha: info.build_sha,
    instance_id: info.instance_id,
  };
}

/** In-flight capture POSTs, drained by flushTelemetry(). */
const pending = new Set<Promise<void>>();

/**
 * Capture one tool call to PostHog. Fire-and-forget — returns immediately and
 * never throws. A no-op when no API key is set.
 */
export function captureToolCall(
  name: string,
  args: Record<string, unknown>,
  result: { isError?: boolean },
): void {
  const docId =
    typeof args?.document_id === "string" && args.document_id
      ? args.document_id
      : undefined;
  captureEvent("mcp_tool_call", {
    tool: name,
    is_error: !!result.isError,
    ...(docId ? { document_id: docId } : {}),
  });
}

/**
 * Capture a named business event (e.g. `fab_handoff_generated`) with explicit
 * aggregate properties. Same rules as captureToolCall: fire-and-forget, never
 * throws, no-op without an API key — and callers must pass only aggregate
 * fields, never argument values or IR.
 */
export function captureEvent(
  event: string,
  eventProperties: Record<string, unknown>,
): void {
  const key = telemetryConfig.apiKey;
  if (!key) return;

  const user = currentUser();
  const docId =
    typeof eventProperties.document_id === "string" && eventProperties.document_id
      ? eventProperties.document_id
      : undefined;

  // Signed-in → attribute to the user (matches the web app's posthog.identify).
  // Anonymous → key by session (document) so events still group meaningfully,
  // but suppress person-profile creation to avoid a person per anon document
  // (matches the web app's `person_profiles: 'identified_only'`).
  const distinctId = user?.sub ?? docId ?? "mcp-anonymous";

  const properties: Record<string, unknown> = {
    ...eventProperties,
    authenticated: !!user,
    ...buildContext,
    $process_person_profile: !!user,
  };
  if (user?.email) properties.$set = { email: user.email };

  const body = JSON.stringify({
    api_key: key,
    event,
    distinct_id: distinctId,
    properties,
    timestamp: new Date().toISOString(),
  });

  const url = `${telemetryConfig.host.replace(/\/$/, "")}/capture/`;
  const p = fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  })
    .then((res) => {
      if (!res.ok) {
        console.error(
          `[mcp] PostHog capture failed: ${res.status} ${res.statusText}`,
        );
      }
    })
    .catch((err) => {
      console.error("[mcp] PostHog capture error:", err);
    })
    .finally(() => {
      pending.delete(p);
    });
  pending.add(p);
}

/**
 * Await all in-flight captures, with a hard timeout so a slow PostHog can never
 * hold a response open. Call after handling each MCP request so events flush
 * before a serverless instance freezes. Never throws.
 */
export async function flushTelemetry(timeoutMs = 2000): Promise<void> {
  if (pending.size === 0) return;
  const inflight = Promise.allSettled([...pending]);
  const timeout = new Promise<void>((resolve) => {
    const t = setTimeout(resolve, timeoutMs);
    if (typeof t.unref === "function") t.unref();
  });
  await Promise.race([inflight, timeout]);
}
