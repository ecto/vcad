// Supabase-backed chat thread persistence.
//
// Threads are scoped to (user_id, document_id). The server (api/chat.ts) is
// the source of truth for messages and tool calls — it writes assistant
// messages, deltas, and tool_call rows as the Anthropic stream arrives. The
// client subscribes via Realtime so multi-tab sessions stay in sync and a
// reload mid-stream can replay the partial response.
//
// User messages and tool results are also persisted by the server (so that
// the row id is atomic with the streaming response), but we provide
// helpers to write them locally too — useful for the welcome / first-paint
// path before the server has written anything.

import { ensureSession, getSupabase } from "./client";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ChatMessageStatus =
  | "pending"
  | "streaming"
  | "complete"
  | "interrupted"
  | "error";

export type ChatToolStatus = "pending" | "success" | "error";

/** Persisted thread row (snake_case columns from Supabase). */
export interface DbChatThread {
  id: string;
  user_id: string;
  document_id: string;
  title: string | null;
  model_id: string | null;
  head_message_id: string | null;
  status: "active" | "archived";
  created_at: string;
  updated_at: string;
  last_activity_at: string;
}

/** Persisted message row. content_blocks is the Anthropic-format payload. */
export interface DbChatMessage {
  id: string;
  thread_id: string;
  parent_id: string | null;
  role: "user" | "assistant";
  content_blocks: unknown[];
  attachments: unknown | null;
  context: unknown | null;
  status: ChatMessageStatus;
  input_tokens: number | null;
  output_tokens: number | null;
  duration_ms: number | null;
  model_id: string | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
}

export interface DbChatToolCall {
  id: string;
  message_id: string;
  thread_id: string;
  name: string;
  args: Record<string, unknown>;
  result: unknown | null;
  status: ChatToolStatus;
  display: unknown | null;
  image_data_url: string | null;
  started_at: string;
  completed_at: string | null;
  duration_ms: number | null;
}

export interface DbChatMessageDelta {
  id: number;
  message_id: string;
  sequence: number;
  delta_type: "text" | "tool_start" | "tool_input_json" | "block_stop" | "done";
  payload: unknown | null;
  created_at: string;
}

export interface ThreadHydration {
  thread: DbChatThread;
  messages: DbChatMessage[];
  toolCalls: DbChatToolCall[];
}

export interface ThreadSubscription {
  unsubscribe: () => void;
}

export interface ThreadSubscriptionCallbacks {
  onMessageInsert?: (msg: DbChatMessage) => void;
  onMessageUpdate?: (msg: DbChatMessage) => void;
  onToolCallInsert?: (tc: DbChatToolCall) => void;
  onToolCallUpdate?: (tc: DbChatToolCall) => void;
  onDeltaInsert?: (delta: DbChatMessageDelta) => void;
}

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

/** Find or create a thread for (current user, document). Returns null when
 * Supabase isn't configured (self-hosted no-auth). */
export async function loadOrCreateThread(
  documentId: string,
): Promise<DbChatThread | null> {
  const supabase = getSupabase();
  if (!supabase) return null;
  const session = await ensureSession();
  if (!session?.user) return null;
  const userId = session.user.id;

  const { data: existing, error: selectError } = await supabase
    .from("chat_threads")
    .select("*")
    .eq("user_id", userId)
    .eq("document_id", documentId)
    .maybeSingle();

  if (selectError) {
    console.warn("[chat-persistence] thread select failed:", selectError.message);
    return null;
  }
  if (existing) return existing as DbChatThread;

  const { data: created, error: insertError } = await supabase
    .from("chat_threads")
    .insert({ user_id: userId, document_id: documentId })
    .select()
    .single();

  if (insertError) {
    // A race might have created the row between select and insert; refetch.
    const { data: refetched } = await supabase
      .from("chat_threads")
      .select("*")
      .eq("user_id", userId)
      .eq("document_id", documentId)
      .maybeSingle();
    if (refetched) return refetched as DbChatThread;
    console.warn("[chat-persistence] thread insert failed:", insertError.message);
    return null;
  }
  return created as DbChatThread;
}

/** Hydrate the full thread state for the current user + document. Also
 * opportunistically calls sweep_orphaned_streams so messages from a dead
 * server function are flipped to interrupted before we render. */
export async function hydrateThread(
  documentId: string,
): Promise<ThreadHydration | null> {
  const supabase = getSupabase();
  if (!supabase) return null;
  const thread = await loadOrCreateThread(documentId);
  if (!thread) return null;

  // Best-effort sweep — ignore errors since hydration must still succeed.
  await supabase.rpc("sweep_orphaned_streams", {
    thread_id_filter: thread.id,
  });

  const [{ data: messages }, { data: toolCalls }] = await Promise.all([
    supabase
      .from("chat_messages")
      .select("*")
      .eq("thread_id", thread.id)
      .order("created_at", { ascending: true }),
    supabase
      .from("chat_tool_calls")
      .select("*")
      .eq("thread_id", thread.id),
  ]);

  return {
    thread,
    messages: (messages ?? []) as DbChatMessage[],
    toolCalls: (toolCalls ?? []) as DbChatToolCall[],
  };
}

// ---------------------------------------------------------------------------
// Streaming delta replay (used on reload mid-stream)
// ---------------------------------------------------------------------------

export async function loadDeltas(
  messageId: string,
): Promise<DbChatMessageDelta[]> {
  const supabase = getSupabase();
  if (!supabase) return [];
  const { data, error } = await supabase
    .from("chat_message_deltas")
    .select("*")
    .eq("message_id", messageId)
    .order("sequence", { ascending: true });
  if (error) {
    console.warn("[chat-persistence] deltas select failed:", error.message);
    return [];
  }
  return (data ?? []) as DbChatMessageDelta[];
}

// ---------------------------------------------------------------------------
// Realtime subscription
// ---------------------------------------------------------------------------

/** Subscribe to all live changes for a thread: messages (insert + update),
 * tool_calls (insert + update), and message_deltas (insert).
 *
 * Returns a handle with `unsubscribe()`. Subscriber callbacks fire
 * out-of-band; the caller is responsible for merging into local state. */
export function subscribeToThread(
  threadId: string,
  callbacks: ThreadSubscriptionCallbacks,
): ThreadSubscription {
  const supabase = getSupabase();
  if (!supabase) {
    return { unsubscribe: () => undefined };
  }

  const channel = supabase.channel(`chat-thread-${threadId}`);

  channel.on(
    "postgres_changes" as never,
    {
      event: "INSERT",
      schema: "public",
      table: "chat_messages",
      filter: `thread_id=eq.${threadId}`,
    },
    (payload: { new: DbChatMessage }) => {
      callbacks.onMessageInsert?.(payload.new);
    },
  );

  channel.on(
    "postgres_changes" as never,
    {
      event: "UPDATE",
      schema: "public",
      table: "chat_messages",
      filter: `thread_id=eq.${threadId}`,
    },
    (payload: { new: DbChatMessage }) => {
      callbacks.onMessageUpdate?.(payload.new);
    },
  );

  channel.on(
    "postgres_changes" as never,
    {
      event: "INSERT",
      schema: "public",
      table: "chat_tool_calls",
      filter: `thread_id=eq.${threadId}`,
    },
    (payload: { new: DbChatToolCall }) => {
      callbacks.onToolCallInsert?.(payload.new);
    },
  );

  channel.on(
    "postgres_changes" as never,
    {
      event: "UPDATE",
      schema: "public",
      table: "chat_tool_calls",
      filter: `thread_id=eq.${threadId}`,
    },
    (payload: { new: DbChatToolCall }) => {
      callbacks.onToolCallUpdate?.(payload.new);
    },
  );

  // Deltas have no thread_id column; subscribe broadly and filter by message_id
  // membership in the caller. Because deltas only exist for in-flight assistant
  // messages, the volume is small.
  channel.on(
    "postgres_changes" as never,
    {
      event: "INSERT",
      schema: "public",
      table: "chat_message_deltas",
    },
    (payload: { new: DbChatMessageDelta }) => {
      callbacks.onDeltaInsert?.(payload.new);
    },
  );

  channel.subscribe();

  return {
    unsubscribe: () => {
      void supabase.removeChannel(channel);
    },
  };
}

// ---------------------------------------------------------------------------
// Tool result submission (client → server)
// ---------------------------------------------------------------------------

/** Update a tool_call row with its execution result. The server's continuation
 * call (the next /api/chat) will read this and include the result in the
 * Anthropic prompt. */
export async function persistToolResult(
  toolCallId: string,
  result: {
    result: unknown;
    status: ChatToolStatus;
    display?: unknown;
    imageDataUrl?: string;
    durationMs?: number;
  },
): Promise<void> {
  const supabase = getSupabase();
  if (!supabase) return;
  const { error } = await supabase
    .from("chat_tool_calls")
    .update({
      result: result.result,
      status: result.status,
      display: result.display ?? null,
      image_data_url: result.imageDataUrl ?? null,
      duration_ms: result.durationMs ?? null,
      completed_at: new Date().toISOString(),
    })
    .eq("id", toolCallId);
  if (error) {
    console.warn(
      "[chat-persistence] tool result update failed:",
      error.message,
    );
  }
}

/** Soft-delete a thread's messages (used by the "clear conversation"
 * button). Keeps the thread row so the next message reuses the same id. */
export async function clearThreadMessages(threadId: string): Promise<void> {
  const supabase = getSupabase();
  if (!supabase) return;
  const { error } = await supabase
    .from("chat_messages")
    .delete()
    .eq("thread_id", threadId);
  if (error) {
    console.warn("[chat-persistence] clear messages failed:", error.message);
    return;
  }
  await supabase
    .from("chat_threads")
    .update({ head_message_id: null, last_activity_at: new Date().toISOString() })
    .eq("id", threadId);
}
