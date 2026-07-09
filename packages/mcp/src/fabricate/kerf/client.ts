/**
 * KerfClient — thin HTTP client for the kerf execution rail's job API.
 *
 * Configuration is env-only: KERF_URL (no default — absent means the rail is
 * off and `available` is false) and optional KERF_API_TOKEN (sent as
 * `Authorization: Bearer`). Every failure mode a caller can't act on —
 * network error, timeout, non-2xx, malformed body — throws
 * {@link KerfUnreachableError} so the adapter can degrade to null cleanly
 * (the generic estimator then covers) instead of poisoning a quote fan-out.
 */

import type {
  ConfiguratorIntent,
  EvidenceBundle,
  JobState,
  OracleClaim,
  VendorQuote,
} from "./contract.js";
import { KERF_JOB_STATES } from "./contract.js";

/** kerf could not be reached or answered unusably — degrade, don't fail. */
export class KerfUnreachableError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "KerfUnreachableError";
  }
}

/** Injectable fetch seam (mirrors store.ts's setFabricateFetch) for tests. */
export let kerfFetch: typeof fetch = (...args) => fetch(...args);
export function setKerfFetch(fn: typeof fetch): void {
  kerfFetch = fn;
}

/** A kerf job snapshot as returned by POST /api/quote and GET /api/jobs/:id. */
export interface KerfJob {
  job_id: string;
  state: JobState;
  quote: VendorQuote | null;
  live_url: string | null;
  evidence: { items: number; claims: OracleClaim[] } | null;
}

/** Scripted quote runs replay an in-memory recording — fast and offline. */
const DEFAULT_QUOTE_TIMEOUT_MS = 45_000;
/**
 * Live quote runs drive a REAL cloud browser through the vendor's
 * configurator (kerf's route sets maxDuration 300: "minutes, not
 * milliseconds"). Aborting at the scripted 45s would orphan a paid Browser
 * Use session mid-run and discard a real vendor price nobody can retrieve,
 * so live mode waits far longer by default.
 */
const DEFAULT_LIVE_QUOTE_TIMEOUT_MS = 120_000;
const DEFAULT_READ_TIMEOUT_MS = 15_000; // job/evidence reads

// ── structural guards (hand-rolled — no new deps) ──────────────────────────

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}

function isMoney(v: unknown): boolean {
  return (
    isRecord(v) &&
    typeof v.currency === "string" &&
    typeof v.amount_minor === "number"
  );
}

function isVendorQuote(v: unknown): v is VendorQuote {
  if (!isRecord(v)) return false;
  return (
    typeof v.quote_id === "string" &&
    typeof v.vendor === "string" &&
    typeof v.intent_hash === "string" &&
    (v.pricing_basis === "estimate" ||
      v.pricing_basis === "quoted" ||
      v.pricing_basis === "binding") &&
    isMoney(v.unit_price) &&
    isMoney(v.total) &&
    typeof v.lead_time_days === "number" &&
    Array.isArray(v.evidence) &&
    Array.isArray(v.notes)
  );
}

function isOracleClaim(v: unknown): v is OracleClaim {
  if (!isRecord(v)) return false;
  return (
    typeof v.oracle === "string" &&
    (v.verdict === "pass" || v.verdict === "fail" || v.verdict === "unverifiable") &&
    Array.isArray(v.evidence)
  );
}

function parseJob(body: unknown): KerfJob {
  if (!isRecord(body)) throw new KerfUnreachableError("kerf: non-object response body");
  const { job_id, state, quote, live_url, evidence } = body;
  if (typeof job_id !== "string" || !job_id) {
    throw new KerfUnreachableError("kerf: response missing job_id");
  }
  if (typeof state !== "string" || !(KERF_JOB_STATES as readonly string[]).includes(state)) {
    throw new KerfUnreachableError(`kerf: response has unknown job state "${String(state)}"`);
  }
  if (quote != null && !isVendorQuote(quote)) {
    throw new KerfUnreachableError("kerf: response quote is malformed");
  }
  if (live_url != null && typeof live_url !== "string") {
    throw new KerfUnreachableError("kerf: response live_url is malformed");
  }
  let ev: KerfJob["evidence"] = null;
  if (evidence != null) {
    if (
      !isRecord(evidence) ||
      typeof evidence.items !== "number" ||
      !Array.isArray(evidence.claims) ||
      !evidence.claims.every(isOracleClaim)
    ) {
      throw new KerfUnreachableError("kerf: response evidence is malformed");
    }
    ev = { items: evidence.items, claims: evidence.claims };
  }
  return {
    job_id,
    state: state as JobState,
    quote: (quote as VendorQuote | null) ?? null,
    live_url: (live_url as string | null) ?? null,
    evidence: ev,
  };
}

function parseEvidenceBundle(body: unknown): EvidenceBundle {
  if (
    !isRecord(body) ||
    typeof body.job_id !== "string" ||
    typeof body.created_at !== "string" ||
    !Array.isArray(body.items) ||
    !Array.isArray(body.claims) ||
    !body.claims.every(isOracleClaim)
  ) {
    throw new KerfUnreachableError("kerf: evidence bundle is malformed");
  }
  return body as unknown as EvidenceBundle;
}

// ── the client ──────────────────────────────────────────────────────────────

export class KerfClient {
  /** True when KERF_URL is configured — the rail's on/off switch. */
  readonly available: boolean;
  private readonly baseUrl: string;
  private readonly token: string | undefined;

  constructor() {
    this.baseUrl = (process.env.KERF_URL ?? "").trim().replace(/\/+$/, "");
    this.available = this.baseUrl.length > 0;
    this.token = process.env.KERF_API_TOKEN || undefined;
  }

  /**
   * Run a vendor quote job: POST /api/quote with `{vendor, intent, mode}`.
   * "scripted" replays kerf's recorded fixture flow (offline, deterministic);
   * "live" drives the vendor's real configurator in a cloud browser — its
   * default timeout is minutes-scale (see DEFAULT_LIVE_QUOTE_TIMEOUT_MS).
   *
   * The intent is serialized VERBATIM — including each file's wire-only
   * `bytes_base64` (kerf's posted-intent API requires the bytes inline,
   * hash-checked against the FileRef sha256 at the door).
   */
  async quote(
    vendor: string,
    intent: ConfiguratorIntent,
    opts?: { mode?: "scripted" | "live"; timeoutMs?: number },
  ): Promise<KerfJob> {
    const mode = opts?.mode ?? "scripted";
    const body = JSON.stringify({ vendor, intent, mode });
    const json = await this.request(
      "POST",
      "/api/quote",
      body,
      opts?.timeoutMs ??
        (mode === "live" ? DEFAULT_LIVE_QUOTE_TIMEOUT_MS : DEFAULT_QUOTE_TIMEOUT_MS),
    );
    return parseJob(json);
  }

  /** Read a job snapshot: GET /api/jobs/:id. */
  async getJob(jobId: string, opts?: { timeoutMs?: number }): Promise<KerfJob> {
    const json = await this.request(
      "GET",
      `/api/jobs/${encodeURIComponent(jobId)}`,
      undefined,
      opts?.timeoutMs ?? DEFAULT_READ_TIMEOUT_MS,
    );
    return parseJob(json);
  }

  /** Read a job's evidence bundle: GET /api/jobs/:id/evidence. */
  async getEvidence(jobId: string, opts?: { timeoutMs?: number }): Promise<EvidenceBundle> {
    const json = await this.request(
      "GET",
      `/api/jobs/${encodeURIComponent(jobId)}/evidence`,
      undefined,
      opts?.timeoutMs ?? DEFAULT_READ_TIMEOUT_MS,
    );
    return parseEvidenceBundle(json);
  }

  private async request(
    method: "GET" | "POST",
    path: string,
    body: string | undefined,
    timeoutMs: number,
  ): Promise<unknown> {
    if (!this.available) {
      throw new KerfUnreachableError("kerf: KERF_URL is not configured");
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const headers: Record<string, string> = { "Content-Type": "application/json" };
      if (this.token) headers.Authorization = `Bearer ${this.token}`;
      const res = await kerfFetch(`${this.baseUrl}${path}`, {
        method,
        headers,
        body,
        signal: controller.signal,
      });
      if (!res.ok) {
        throw new KerfUnreachableError(`kerf: ${method} ${path} → HTTP ${res.status}`);
      }
      try {
        return await res.json();
      } catch (err) {
        throw new KerfUnreachableError(`kerf: ${method} ${path} → non-JSON body`, {
          cause: err,
        });
      }
    } catch (err) {
      if (err instanceof KerfUnreachableError) throw err;
      throw new KerfUnreachableError(
        `kerf: ${method} ${path} failed (${err instanceof Error ? err.message : String(err)})`,
        { cause: err },
      );
    } finally {
      clearTimeout(timer);
    }
  }
}
