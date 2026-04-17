// Server-side persistence for chat threads. Mirrors the client's
// expectations from packages/auth/src/chat-persistence.ts: writes the user
// message + assistant message rows around each Anthropic call, streams
// deltas into chat_message_deltas, normalizes tool_use blocks into
// chat_tool_calls.
//
// Errors are non-fatal — the client SSE stream is the user's source of truth
// for the active turn. Persistence failures are logged but never block the
// response.

import type { SupabaseClient } from "@supabase/supabase-js";

interface ContentBlock {
  type: string;
  text?: string;
  id?: string;
  name?: string;
  input?: unknown;
  source?: { type: string; media_type: string; data: string };
  tool_use_id?: string;
  content?: unknown;
}

export interface PersistedTurn {
  threadId: string;
  userMessageId: string;
  assistantMessageId: string;
  parentMessageId: string | null;
}

/** Find or create a thread. Returns null on error or when admin is null. */
export async function findOrCreateThread(
  admin: SupabaseClient,
  userId: string,
  documentId: string,
): Promise<{ id: string; head_message_id: string | null } | null> {
  const { data: existing } = await admin
    .from("chat_threads")
    .select("id, head_message_id")
    .eq("user_id", userId)
    .eq("document_id", documentId)
    .maybeSingle();
  if (existing) return existing as { id: string; head_message_id: string | null };

  const { data: created, error } = await admin
    .from("chat_threads")
    .insert({ user_id: userId, document_id: documentId })
    .select("id, head_message_id")
    .single();
  if (error) {
    console.error("[chat-persistence] thread insert failed:", error.message);
    return null;
  }
  return created as { id: string; head_message_id: string | null };
}

/** Write the user message row for this turn. Detects tool-result-only
 * messages (the synthetic continuation after a client-side tool execution)
 * and skips the row entirely — those aren't UI messages, the result lives
 * on the chat_tool_calls row. */
export async function persistUserMessage(
  admin: SupabaseClient,
  args: {
    threadId: string;
    messageId: string;
    parentMessageId: string | null;
    content: string | unknown[];
    attachments?: unknown;
    context?: unknown;
  },
): Promise<{ skipped: boolean }> {
  const blocks = normalizeUserContent(args.content);
  if (isToolResultOnly(blocks)) {
    // Tool-result continuation. Don't insert a UI-visible message; the
    // tool_call row already carries the result.
    return { skipped: true };
  }

  const { error } = await admin.from("chat_messages").upsert(
    {
      id: args.messageId,
      thread_id: args.threadId,
      parent_id: args.parentMessageId,
      role: "user",
      content_blocks: blocks,
      attachments: args.attachments ?? null,
      context: args.context ?? null,
      status: "complete",
      completed_at: new Date().toISOString(),
    },
    { onConflict: "id" },
  );
  if (error) {
    console.error("[chat-persistence] user message upsert failed:", error.message);
  }
  return { skipped: false };
}

/** Insert the assistant message row at status='streaming' before the
 * Anthropic call begins. Updated to status='complete' (or 'interrupted' /
 * 'error') as the stream finalizes. */
export async function persistAssistantStub(
  admin: SupabaseClient,
  args: {
    threadId: string;
    messageId: string;
    parentMessageId: string | null;
    modelId: string;
  },
): Promise<void> {
  const { error } = await admin.from("chat_messages").insert({
    id: args.messageId,
    thread_id: args.threadId,
    parent_id: args.parentMessageId,
    role: "assistant",
    content_blocks: [],
    status: "streaming",
    model_id: args.modelId,
  });
  if (error) {
    console.error("[chat-persistence] assistant stub insert failed:", error.message);
  }
}

/** Finalize an assistant message: write its full content_blocks, status,
 * tokens, duration. */
export async function finalizeAssistantMessage(
  admin: SupabaseClient,
  args: {
    messageId: string;
    contentBlocks: unknown[];
    status: "complete" | "interrupted" | "error";
    inputTokens: number;
    outputTokens: number;
    durationMs: number;
  },
): Promise<void> {
  const { error } = await admin
    .from("chat_messages")
    .update({
      content_blocks: args.contentBlocks,
      status: args.status,
      input_tokens: args.inputTokens,
      output_tokens: args.outputTokens,
      duration_ms: args.durationMs,
      completed_at: new Date().toISOString(),
    })
    .eq("id", args.messageId);
  if (error) {
    console.error("[chat-persistence] finalize assistant failed:", error.message);
  }
}

/** Update head_message_id + last_activity_at after a turn completes. */
export async function updateThreadHead(
  admin: SupabaseClient,
  threadId: string,
  headMessageId: string,
): Promise<void> {
  const { error } = await admin
    .from("chat_threads")
    .update({
      head_message_id: headMessageId,
      last_activity_at: new Date().toISOString(),
    })
    .eq("id", threadId);
  if (error) {
    console.error("[chat-persistence] thread head update failed:", error.message);
  }
}

/** Insert a tool_call row when Anthropic emits a tool_use block start.
 * Status starts pending; the client posts the result back via
 * persistToolResult (see packages/auth/src/chat-persistence.ts). */
export async function persistToolCallStart(
  admin: SupabaseClient,
  args: {
    toolUseId: string;
    messageId: string;
    threadId: string;
    name: string;
  },
): Promise<void> {
  const { error } = await admin.from("chat_tool_calls").insert({
    id: args.toolUseId,
    message_id: args.messageId,
    thread_id: args.threadId,
    name: args.name,
    args: {},
    status: "pending",
  });
  if (error) {
    console.error("[chat-persistence] tool_call insert failed:", error.message);
  }
}

/** Update args jsonb on a tool_call once the JSON has been fully streamed. */
export async function persistToolCallArgs(
  admin: SupabaseClient,
  toolUseId: string,
  parsedArgs: Record<string, unknown>,
): Promise<void> {
  const { error } = await admin
    .from("chat_tool_calls")
    .update({ args: parsedArgs })
    .eq("id", toolUseId);
  if (error) {
    console.error("[chat-persistence] tool_call args update failed:", error.message);
  }
}

/** Append a delta row. Sequence is generated by the caller (monotonic per
 * message_id). Fire-and-forget; we don't await per-delta. */
export async function persistDelta(
  admin: SupabaseClient,
  args: {
    messageId: string;
    sequence: number;
    deltaType: "text" | "tool_start" | "tool_input_json" | "block_stop" | "done";
    payload: unknown;
  },
): Promise<void> {
  const { error } = await admin
    .from("chat_message_deltas")
    .insert({
      message_id: args.messageId,
      sequence: args.sequence,
      delta_type: args.deltaType,
      payload: args.payload,
    });
  if (error) {
    // Common case: unique-violation if the same sequence retried — ignore.
    if (!error.message.includes("duplicate key")) {
      console.warn("[chat-persistence] delta insert:", error.message);
    }
  }
}

// ---------------------------------------------------------------------------
// Reading thread state for prompt assembly (used when /api/chat is called
// with a thread_id continuation that doesn't include the full history)
// ---------------------------------------------------------------------------

export interface AssembledPromptMessage {
  role: "user" | "assistant";
  content: ContentBlock[];
}

/** Read the full thread state and assemble it into Anthropic prompt format.
 * Walks chat_messages in created_at order; for each assistant message with
 * tool_use blocks, synthesizes a follow-up user message containing
 * tool_result blocks built from the corresponding chat_tool_calls rows
 * (only those with status != 'pending'). */
export async function assemblePromptFromThread(
  admin: SupabaseClient,
  threadId: string,
): Promise<AssembledPromptMessage[]> {
  const [{ data: msgs }, { data: tools }] = await Promise.all([
    admin
      .from("chat_messages")
      .select("id, role, content_blocks, attachments, status")
      .eq("thread_id", threadId)
      .order("created_at", { ascending: true }),
    admin
      .from("chat_tool_calls")
      .select("id, message_id, result, status")
      .eq("thread_id", threadId),
  ]);

  if (!msgs) return [];

  const toolsByMessage = new Map<string, Array<{ id: string; result: unknown; status: string }>>();
  for (const t of (tools ?? []) as Array<{ id: string; message_id: string; result: unknown; status: string }>) {
    const arr = toolsByMessage.get(t.message_id) ?? [];
    arr.push({ id: t.id, result: t.result, status: t.status });
    toolsByMessage.set(t.message_id, arr);
  }

  const out: AssembledPromptMessage[] = [];
  for (const m of msgs as Array<{
    id: string;
    role: "user" | "assistant";
    content_blocks: ContentBlock[];
    status: string;
  }>) {
    // Skip messages that never produced output (interrupted/error stubs).
    if (
      m.role === "assistant" &&
      (!m.content_blocks || m.content_blocks.length === 0)
    ) {
      continue;
    }
    out.push({ role: m.role, content: m.content_blocks ?? [] });

    if (m.role === "assistant") {
      const toolUses = (m.content_blocks ?? []).filter(
        (b) => b.type === "tool_use",
      );
      if (toolUses.length === 0) continue;
      const tcByMsg = toolsByMessage.get(m.id) ?? [];
      const results: ContentBlock[] = [];
      for (const tu of toolUses) {
        if (!tu.id) continue;
        const tc = tcByMsg.find((t) => t.id === tu.id);
        if (!tc || tc.status === "pending") continue;
        results.push({
          type: "tool_result",
          tool_use_id: tu.id,
          content: stringifyToolResult(tc.result),
        });
      }
      if (results.length > 0) {
        out.push({ role: "user", content: results });
      }
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function normalizeUserContent(content: string | unknown[]): ContentBlock[] {
  if (typeof content === "string") {
    if (!content) return [];
    return [{ type: "text", text: content }];
  }
  return content as ContentBlock[];
}

function isToolResultOnly(blocks: ContentBlock[]): boolean {
  if (blocks.length === 0) return false;
  return blocks.every((b) => b.type === "tool_result");
}

function stringifyToolResult(result: unknown): string {
  if (result === null || result === undefined) return "";
  if (typeof result === "string") return result;
  return JSON.stringify(result);
}
