/**
 * Vercel Edge Config reader — lets WARM serverless instances pick up runtime
 * config (feature flags, the latest-deployed git sha) WITHOUT a redeploy.
 *
 * The warm-instance trap (see the `mcp-deploy-and-warm-instances` note): after
 * an env-var change + redeploy, an existing MCP connection can stay pinned to a
 * pre-deploy warm lambda that never saw the new env — so a freshly-enabled flag
 * (e.g. VCAD_LIVE_WINDOW) looks "not enabled" over that connection while
 * `curl /health` on a fresh instance reports it on. Baked-in env vars and
 * `--define` constants can't fix that: they're frozen at the instance's cold
 * start. Edge Config is a globally replicated, low-latency KV that every
 * instance reads at RUNTIME, so flipping a value in the Vercel dashboard reaches
 * every warm lambda within `TTL_MS` — no redeploy, no drain.
 *
 * Two uses here:
 *   1. Feature flags (e.g. VCAD_LIVE_WINDOW) — `getRuntimeFlag()` reads Edge
 *      Config first, the process env var second. Warm instances honor a flipped
 *      flag on the next request.
 *   2. Staleness — the deploy step (services/mcp/build.sh) publishes the new
 *      commit's sha as `expected_build_sha`; a running instance compares it to
 *      its own baked BUILD_SHA to know whether it is the current build
 *      (`getExpectedBuildSha()` + `computeStaleness()`).
 *
 * Graceful degradation: with EDGE_CONFIG unset (local dev, stdio, Fly.io), every
 * read falls straight through to the process env var. No network, no behavior
 * change from the pre-Edge-Config world.
 */

// Injectable fetch seam — mirrors session-store.ts's `setSessionFetch` so Edge
// Config reads are unit-testable without a network.
let edgeFetch: typeof fetch = (...args) => fetch(...args);
/** Test hook — swap in a fake fetch for the Edge Config `/items` endpoint. */
export function setEdgeConfigFetch(fn: typeof fetch): void {
  edgeFetch = fn;
}
/** Test hook — drop the TTL cache so a test can flip a value between reads. */
export function resetEdgeConfigCache(): void {
  cache = null;
  inflight = null;
}

/** How long a fetched snapshot is trusted before the next read re-fetches. A
 *  burst of tool calls in one connection thus makes at most one Edge Config
 *  request per window; a flag flip propagates to warm instances within it. */
const TTL_MS: number = (() => {
  const n = Number.parseInt(process.env.VCAD_EDGE_CONFIG_TTL_MS ?? "", 10);
  return Number.isFinite(n) && n >= 0 ? n : 5_000;
})();

let cache: { at: number; items: Record<string, unknown> } | null = null;
let inflight: Promise<Record<string, unknown> | null> | null = null;

/** Derive the read-all `/items` endpoint from the EDGE_CONFIG connection string
 *  (`https://edge-config.vercel.com/<id>?token=<read-token>`). Returns null when
 *  unset or malformed — the caller then runs in env-only mode. */
function itemsUrl(): string | null {
  const conn = process.env.EDGE_CONFIG;
  if (!conn) return null;
  try {
    const u = new URL(conn);
    const token = u.searchParams.get("token");
    if (!token) return null;
    const base = u.pathname.replace(/\/$/, "");
    return `${u.origin}${base}/items?token=${encodeURIComponent(token)}`;
  } catch {
    return null;
  }
}

/** Read every Edge Config item, TTL-cached and single-flight. Never throws and
 *  never stalls a request: on timeout/error it serves the last good snapshot
 *  (or null), so a slow Edge read degrades to env-only, it doesn't break a call. */
async function readItems(): Promise<Record<string, unknown> | null> {
  const url = itemsUrl();
  if (!url) return null; // not configured → env-only mode
  const now = Date.now();
  if (cache && now - cache.at < TTL_MS) return cache.items;
  if (inflight) return inflight; // coalesce concurrent reads within a window
  inflight = (async () => {
    try {
      const res = await edgeFetch(url, {
        signal: AbortSignal.timeout(2_000),
      } as RequestInit);
      if (!res.ok) return cache?.items ?? null;
      const items = (await res.json()) as Record<string, unknown>;
      cache = { at: Date.now(), items };
      return items;
    } catch {
      return cache?.items ?? null; // serve stale on any error
    } finally {
      inflight = null;
    }
  })();
  return inflight;
}

function coerceBool(v: unknown): boolean | null {
  if (typeof v === "boolean") return v;
  if (typeof v === "number") return v !== 0;
  if (typeof v === "string") {
    const s = v.trim().toLowerCase();
    if (s === "1" || s === "true" || s === "on" || s === "yes") return true;
    if (s === "0" || s === "false" || s === "off" || s === "no") return false;
  }
  return null;
}

/**
 * Runtime feature flag — Edge Config first (keys `flag.<NAME>` or `<NAME>`),
 * process env (`<NAME>` truthy when `"1"`) second. Async because Edge Config is
 * a network read; when EDGE_CONFIG is unset it resolves immediately off the env,
 * so flag semantics are unchanged where Edge Config isn't wired up.
 */
export async function getRuntimeFlag(name: string): Promise<boolean> {
  const items = await readItems();
  if (items) {
    const fromEdge = coerceBool(items[`flag.${name}`] ?? items[name]);
    if (fromEdge !== null) return fromEdge;
  }
  return process.env[name] === "1";
}

/**
 * The git sha of the latest deployment, published to Edge Config by the deploy
 * step. A running instance compares it to its own baked BUILD_SHA to know if a
 * newer build exists. null when nothing is published (Edge Config unset or key
 * absent) — staleness is then simply unknown and never asserted.
 */
export async function getExpectedBuildSha(): Promise<string | null> {
  const items = await readItems();
  const v = items?.["expected_build_sha"] ?? items?.["latest_build_sha"];
  if (typeof v === "string" && v.trim()) return v.trim();
  const env = process.env.VCAD_EXPECTED_BUILD_SHA;
  return env && env.trim() ? env.trim() : null;
}

/**
 * Compare a running build sha against the latest published sha. `is_stale` is
 * only ever TRUE when BOTH are known AND differ — an unknown expected sha (Edge
 * Config unset / key absent / sentinel) yields `is_stale:false` so we never cry
 * wolf on hosts that don't publish one. Short/full sha mismatches (a 7-char
 * `expected` vs a 40-char running sha, or vice-versa) count as the same build.
 */
export function computeStaleness(
  runningSha: string,
  expectedSha: string | null,
): { expected_build_sha: string | null; is_stale: boolean } {
  const known =
    !!expectedSha &&
    expectedSha !== "unknown" &&
    !!runningSha &&
    runningSha !== "unknown";
  const same =
    known &&
    (expectedSha === runningSha ||
      runningSha.startsWith(expectedSha as string) ||
      (expectedSha as string).startsWith(runningSha));
  return {
    expected_build_sha: expectedSha,
    is_stale: known && !same,
  };
}

/** Convenience: read the expected sha and compare in one call. */
export async function getStaleness(
  runningSha: string,
): Promise<{ expected_build_sha: string | null; is_stale: boolean }> {
  return computeStaleness(runningSha, await getExpectedBuildSha());
}
