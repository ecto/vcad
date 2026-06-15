/**
 * Discord activity rollups for MCP tool usage.
 *
 * Posts a periodic summary to a Discord channel so we can see at a glance that
 * a deployed vcad MCP server is being used — e.g. "23 tool calls across 4
 * sessions" every 15 minutes — rather than a line per call. Disabled until a
 * webhook URL is set in `notifyConfig` below, so local/dev runs and the test
 * suite stay silent and offline.
 *
 * Design constraints:
 *   - Never block the tool response. Aggregation is in-memory and O(1); the
 *     rollup POST is fire-and-forget on a background timer.
 *   - Never throw into the caller. Every failure is swallowed to stderr.
 *   - Stay quiet when idle. Empty windows are skipped (no "0 calls" pings),
 *     and the timer stops itself until the next call arrives.
 *   - Don't leak payloads. Only tool names, call counts, error counts, and the
 *     number of distinct sessions are sent — never argument values or IR.
 */

/**
 * Notifier configuration. No env vars — paste the internal Discord webhook
 * URL here to enable rollups. NOTE: this repo is public, so a URL committed
 * here lands in git history; treat it as a low-value secret (post-only, one
 * channel) and rotate it from Discord if needed.
 */
export const notifyConfig = {
  /** Internal Discord webhook URL. Empty = rollups disabled. */
  webhookUrl: "",
  /** Rollup interval in milliseconds (default 15 minutes). */
  rollupMs: 15 * 60 * 1000,
  /** Webhook display name. */
  username: "vcad mcp",
};

/** Discord hard-caps message content at 2000 chars; stay comfortably below. */
const MAX_CONTENT = 1800;
/** Most tools to itemize in the rollup before collapsing into "+N more". */
const MAX_TOOLS = 15;

/** Mutable counters for the current rollup window. */
interface Window {
  total: number;
  errors: number;
  perTool: Map<string, number>;
  sessions: Set<string>;
  startedAt: number;
}

let win: Window | null = null;
let timer: ReturnType<typeof setInterval> | null = null;

/** Truncate a string to `n` chars with an ellipsis. */
function clip(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

function plural(n: number): string {
  return n === 1 ? "" : "s";
}

function freshWindow(): Window {
  return {
    total: 0,
    errors: 0,
    perTool: new Map(),
    sessions: new Set(),
    startedAt: Date.now(),
  };
}

/**
 * Record a tool call toward the current rollup window. Fire-and-forget —
 * returns immediately and never throws. A no-op when no webhook URL is set.
 */
export function fireToolAlert(
  name: string,
  args: Record<string, unknown>,
  result: { isError?: boolean },
): void {
  if (!notifyConfig.webhookUrl) return;

  if (!win) win = freshWindow();
  win.total += 1;
  if (result.isError) win.errors += 1;
  win.perTool.set(name, (win.perTool.get(name) ?? 0) + 1);
  const docId = args?.document_id;
  if (typeof docId === "string" && docId) win.sessions.add(docId);

  // Start ticking on the first call; the timer stops itself once idle.
  if (!timer) {
    timer = setInterval(rollup, notifyConfig.rollupMs);
    // Don't keep the process alive just to deliver a rollup.
    if (typeof timer.unref === "function") timer.unref();
  }
}

/** Build the human-readable rollup message body for a window. */
function formatRollup(w: Window): string {
  const mins = Math.max(1, Math.round((Date.now() - w.startedAt) / 60_000));

  const sessions = w.sessions.size > 0
    ? ` across ${w.sessions.size} session${plural(w.sessions.size)}`
    : "";
  const errors = w.errors > 0 ? ` · ${w.errors} error${plural(w.errors)}` : "";

  const ranked = [...w.perTool.entries()].sort((a, b) => b[1] - a[1]);
  const shown = ranked.slice(0, MAX_TOOLS);
  let tools = shown.map(([t, n]) => `\`${t}\` ×${n}`).join(" · ");
  if (ranked.length > shown.length) {
    tools += ` · +${ranked.length - shown.length} more`;
  }

  return clip(
    `📊 **vcad activity** · last ${mins}m\n` +
      `${w.total} tool call${plural(w.total)}${sessions}${errors}\n` +
      tools,
    MAX_CONTENT,
  );
}

/** Timer tick: post the window's summary, or stop ticking if idle. */
async function rollup(): Promise<void> {
  const url = notifyConfig.webhookUrl;
  const w = win;

  // Idle window: nothing to report — stop the timer until the next call.
  if (!w || w.total === 0 || !url) {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
    win = null;
    return;
  }

  // Start a fresh window immediately so calls during the POST aren't lost.
  win = null;

  const body = JSON.stringify({
    username: notifyConfig.username,
    content: formatRollup(w),
    allowed_mentions: { parse: [] },
  });

  try {
    const res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
    });
    if (!res.ok) {
      console.error(
        `[mcp] Discord rollup failed: ${res.status} ${res.statusText}`,
      );
    }
  } catch (err) {
    console.error("[mcp] Discord rollup error:", err);
  }
}
