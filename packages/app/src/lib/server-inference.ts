/**
 * Server-based inference for text-to-CAD generation.
 *
 * Proxies through /api/generate which validates Supabase auth
 * and calls HuggingFace Inference Endpoint.
 */

/** API base URL - defaults to same origin */
const API_BASE = import.meta.env.VITE_API_URL || "";

/** Inference result. */
export interface ServerInferResult {
  ir: string;
  tokens: number;
  durationMs: number;
  /** ID of the inference log entry (for rating) */
  logId?: string;
}

/**
 * Generate VCode from a text prompt using server inference.
 *
 * @param prompt - Text description of the desired CAD part
 * @param options - Generation options (authToken required for Supabase auth)
 * @returns The generated VCode
 */
export async function generateCADServer(
  prompt: string,
  options: {
    /** Supabase auth token (required) */
    authToken: string;
    /** Optional abort signal to cancel the request */
    signal?: AbortSignal;
  }
): Promise<ServerInferResult> {
  const response = await fetch(`${API_BASE}/api/generate`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${options.authToken}`,
    },
    body: JSON.stringify({ prompt }),
    signal: options.signal,
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({ error: "Request failed" }));
    throw new Error(data.error || `Server inference failed: ${response.status}`);
  }

  const result = await response.json();

  if (result.error) {
    throw new Error(result.error);
  }

  return {
    ir: result.ir ?? "",
    tokens: result.tokens ?? 0,
    durationMs: result.durationMs ?? 0,
    logId: result.logId,
  };
}

/**
 * Submit a rating for a generated result.
 *
 * @param logId - The inference log ID
 * @param rating - Rating value: -1 (bad), 0 (neutral), 1 (good)
 * @param authToken - Supabase auth token
 */
export async function rateGeneration(
  logId: string,
  rating: -1 | 0 | 1,
  authToken: string
): Promise<void> {
  const response = await fetch(`${API_BASE}/api/rate`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${authToken}`,
    },
    body: JSON.stringify({ logId, rating }),
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({ error: "Request failed" }));
    throw new Error(data.error || `Rating failed: ${response.status}`);
  }
}
