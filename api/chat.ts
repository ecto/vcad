import type { VercelRequest, VercelResponse } from "@vercel/node";
import { createClient, type SupabaseClient } from "@supabase/supabase-js";
import { createHash } from "node:crypto";

const FALLBACK_SYSTEM_PROMPT =
  "You are vcad's AI assistant — a parametric CAD copilot. Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters. Be concise.";

const ANON_DAILY_LIMIT = 3;
const MONTHLY_TOKEN_LIMIT = 500_000;
const ANTHROPIC_MODEL = "claude-sonnet-4-6";
const ANTHROPIC_MAX_TOKENS = 8192;

type AnthropicTool = {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
};

type ChatRequestBody = {
  messages: Array<{ role: "user" | "assistant"; content: string | object[] }>;
  context?: { selectedParts: Array<{ partId: string; partName: string; geometryType: string }> };
  tools?: AnthropicTool[];
  systemPrompt?: string;
};

// ---------------------------------------------------------------------------
// Supabase helpers
// ---------------------------------------------------------------------------

let cachedAdmin: SupabaseClient | null = null;
function getSupabaseAdmin(): SupabaseClient | null {
  if (cachedAdmin) return cachedAdmin;
  const url = process.env.SUPABASE_URL;
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY;
  if (!url || !key) return null;
  cachedAdmin = createClient(url, key, { auth: { persistSession: false } });
  return cachedAdmin;
}

function getClientIp(req: VercelRequest): string {
  const xff = req.headers["x-forwarded-for"];
  if (typeof xff === "string" && xff.length > 0) {
    return xff.split(",")[0]!.trim();
  }
  if (Array.isArray(xff) && xff.length > 0) {
    return xff[0]!.split(",")[0]!.trim();
  }
  const cf = req.headers["cf-connecting-ip"];
  if (typeof cf === "string") return cf;
  return req.socket?.remoteAddress ?? "unknown";
}

function hashIp(ip: string): string {
  const salt = process.env.IP_HASH_SALT ?? "vcad-default-salt-change-me";
  return createHash("sha256").update(`${salt}:${ip}`).digest("hex");
}

async function getUserIdFromAuth(
  req: VercelRequest,
  admin: SupabaseClient | null,
): Promise<string | null> {
  if (!admin) return null;
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith("Bearer ")) return null;
  const token = authHeader.slice(7);
  try {
    const { data, error } = await admin.auth.getUser(token);
    if (error || !data.user) return null;
    return data.user.id;
  } catch {
    return null;
  }
}

async function countAnonMessages(admin: SupabaseClient, ipHash: string): Promise<number> {
  const oneDayAgo = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();
  const { count, error } = await admin
    .from("inference_logs")
    .select("id", { count: "exact", head: true })
    .eq("ip_hash", ipHash)
    .eq("kind", "chat")
    .gte("created_at", oneDayAgo);
  if (error) {
    console.error("[chat] countAnonMessages error:", error);
    return 0;
  }
  return count ?? 0;
}

async function sumMonthlyTokens(admin: SupabaseClient, userId: string): Promise<number> {
  const startOfMonth = new Date();
  startOfMonth.setUTCDate(1);
  startOfMonth.setUTCHours(0, 0, 0, 0);
  const { data, error } = await admin
    .from("inference_logs")
    .select("tokens")
    .eq("user_id", userId)
    .eq("kind", "chat")
    .gte("created_at", startOfMonth.toISOString());
  if (error) {
    console.error("[chat] sumMonthlyTokens error:", error);
    return 0;
  }
  return (data ?? []).reduce((sum, row) => sum + ((row.tokens as number | null) ?? 0), 0);
}

function nextMonthStartIso(): string {
  const d = new Date();
  const next = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1, 0, 0, 0));
  return next.toISOString();
}

async function logUsage(
  admin: SupabaseClient,
  userId: string | null,
  ipHash: string | null,
  promptPreview: string,
  tokens: number,
  toolCalls: number,
  durationMs: number,
  error: string | null,
): Promise<void> {
  const { error: insertError } = await admin.from("inference_logs").insert({
    kind: "chat",
    user_id: userId,
    ip_hash: userId ? null : ipHash,
    prompt: promptPreview,
    result: null,
    tokens,
    tool_calls: toolCalls,
    duration_ms: durationMs,
    error,
  });
  if (insertError) console.error("[chat] insert log failed:", insertError);
}

// ---------------------------------------------------------------------------
// Anthropic SSE → client streaming format
// ---------------------------------------------------------------------------

/**
 * Stream Anthropic's SSE response and translate it into the simpler newline-
 * delimited JSON format that the vcad client expects:
 *   data: { type: "text" | "tool_start" | "tool_delta" | "block_stop" | "done" }
 *
 * Returns { tokens, toolCallCount } for usage logging.
 */
async function pipeAnthropicStream(
  anthropicBody: ReadableStream<Uint8Array>,
  write: (chunk: string) => void,
): Promise<{ tokens: number; toolCallCount: number }> {
  const reader = anthropicBody.getReader();
  const decoder = new TextDecoder();
  let sseBuffer = "";
  let tokens = 0;
  let toolCallCount = 0;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    sseBuffer += decoder.decode(value, { stream: true });
    const lines = sseBuffer.split("\n");
    sseBuffer = lines.pop() ?? "";
    for (const line of lines) {
      if (!line.startsWith("data: ")) continue;
      const data = line.slice(6);
      if (data === "[DONE]") continue;
      try {
        const event = JSON.parse(data);
        if (event.type === "content_block_delta" && event.delta?.type === "text_delta") {
          write(`data: ${JSON.stringify({ type: "text", text: event.delta.text })}\n\n`);
        } else if (event.type === "content_block_start" && event.content_block?.type === "tool_use") {
          toolCallCount++;
          write(
            `data: ${JSON.stringify({
              type: "tool_start",
              id: event.content_block.id,
              name: event.content_block.name,
            })}\n\n`,
          );
        } else if (event.type === "content_block_delta" && event.delta?.type === "input_json_delta") {
          write(`data: ${JSON.stringify({ type: "tool_delta", json: event.delta.partial_json })}\n\n`);
        } else if (event.type === "content_block_stop") {
          write(`data: ${JSON.stringify({ type: "block_stop" })}\n\n`);
        } else if (event.type === "message_delta" && event.usage) {
          // Anthropic emits cumulative usage in message_delta
          const u = event.usage;
          tokens = (u.input_tokens ?? 0) + (u.output_tokens ?? 0);
        } else if (event.type === "message_stop") {
          write(`data: ${JSON.stringify({ type: "done" })}\n\n`);
        }
      } catch {
        /* skip non-JSON SSE comments, etc. */
      }
    }
  }

  return { tokens, toolCallCount };
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

export default async function handler(req: VercelRequest, res: VercelResponse) {
  // CORS
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization");

  if (req.method === "OPTIONS") {
    res.status(200).end();
    return;
  }

  if (req.method !== "POST") {
    res.status(405).json({ error: "Method not allowed" });
    return;
  }

  // req.body may be a parsed object (Vercel) or a string (dev Node http).
  let body: ChatRequestBody;
  if (typeof req.body === "string") {
    try { body = JSON.parse(req.body); } catch { res.status(400).json({ error: "invalid json" }); return; }
  } else if (req.body && typeof req.body === "object") {
    body = req.body as ChatRequestBody;
  } else {
    // Read raw body (dev path when no body parser ran)
    let raw = "";
    for await (const chunk of req) raw += chunk;
    try { body = JSON.parse(raw); } catch { res.status(400).json({ error: "invalid json" }); return; }
  }

  const { messages, tools: clientTools, systemPrompt: clientSystemPrompt } = body;

  if (!messages?.length) {
    res.status(400).json({ error: "messages required" });
    return;
  }

  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    res.status(503).json({ error: "Chat service not configured (missing ANTHROPIC_API_KEY)" });
    return;
  }

  const admin = getSupabaseAdmin();
  const userId = await getUserIdFromAuth(req, admin);
  const ip = getClientIp(req);
  const ipHash = hashIp(ip);

  // Rate limit checks — only enforced when Supabase is configured.
  if (admin) {
    if (userId) {
      const used = await sumMonthlyTokens(admin, userId);
      if (used >= MONTHLY_TOKEN_LIMIT) {
        res.status(429).json({
          error: "monthly_limit",
          message: `Monthly chat limit reached (${used.toLocaleString()} of ${MONTHLY_TOKEN_LIMIT.toLocaleString()} tokens). Resets at the start of next month.`,
          usage: used,
          limit: MONTHLY_TOKEN_LIMIT,
          resets_at: nextMonthStartIso(),
        });
        return;
      }
    } else {
      const used = await countAnonMessages(admin, ipHash);
      if (used >= ANON_DAILY_LIMIT) {
        res.status(429).json({
          error: "anon_limit",
          message: `You've used your ${ANON_DAILY_LIMIT} free chat messages. Sign in for more.`,
          usage: used,
          limit: ANON_DAILY_LIMIT,
        });
        return;
      }
    }
  }

  const systemPrompt = clientSystemPrompt || FALLBACK_SYSTEM_PROMPT;
  const tools = clientTools || [];
  const startedAt = Date.now();

  try {
    const anthropicRes = await fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-api-key": apiKey,
        "anthropic-version": "2023-06-01",
      },
      body: JSON.stringify({
        model: ANTHROPIC_MODEL,
        max_tokens: ANTHROPIC_MAX_TOKENS,
        system: systemPrompt,
        stream: true,
        tools,
        messages,
      }),
    });

    if (!anthropicRes.ok) {
      const errText = await anthropicRes.text();
      res.statusCode = anthropicRes.status;
      res.end(errText);
      // Log the error for visibility but don't block on it
      if (admin) {
        const promptPreview = extractPromptPreview(messages);
        void logUsage(admin, userId, ipHash, promptPreview, 0, 0, Date.now() - startedAt, errText.slice(0, 500));
      }
      return;
    }

    // Client-compatible streaming response
    res.setHeader("Content-Type", "text/event-stream; charset=utf-8");
    res.setHeader("Transfer-Encoding", "chunked");
    res.setHeader("Cache-Control", "no-cache");

    if (!anthropicRes.body) {
      res.end();
      return;
    }

    const { tokens, toolCallCount } = await pipeAnthropicStream(
      anthropicRes.body,
      (chunk) => res.write(chunk),
    );
    res.end();

    // Fire-and-forget usage log after the stream closes.
    if (admin) {
      const promptPreview = extractPromptPreview(messages);
      void logUsage(
        admin,
        userId,
        ipHash,
        promptPreview,
        tokens,
        toolCallCount,
        Date.now() - startedAt,
        null,
      );
    }
  } catch (err) {
    console.error("Chat API error:", err);
    if (!res.headersSent) {
      res.status(500).json({ error: "Internal server error" });
    } else {
      try { res.end(); } catch { /* noop */ }
    }
  }
}

function extractPromptPreview(
  messages: Array<{ role: "user" | "assistant"; content: string | object[] }>,
): string {
  const lastUser = [...messages].reverse().find((m) => m.role === "user");
  if (!lastUser) return "";
  if (typeof lastUser.content === "string") return lastUser.content.slice(0, 2000);
  return JSON.stringify(lastUser.content).slice(0, 2000);
}
