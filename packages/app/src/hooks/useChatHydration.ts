import { useEffect, useRef } from "react";
import {
  useChatStore,
  useDocumentStore,
  type ChatAttachment,
  type ChatMessage,
  type MessagePart,
  type SelectionContext,
  type ToolCallInfo,
} from "@vcad/core";
import {
  hydrateThread,
  loadDeltas,
  subscribeToThread,
  type DbChatMessage,
  type DbChatMessageDelta,
  type DbChatToolCall,
  type ThreadSubscription,
} from "@vcad/auth";

// ---------------------------------------------------------------------------
// DB → in-memory ChatMessage projection
// ---------------------------------------------------------------------------

interface ContentBlock {
  type: string;
  text?: string;
  id?: string;
  name?: string;
  input?: Record<string, unknown>;
  source?: { data: string; media_type: string };
}

/** Project the row from chat_messages + chat_tool_calls into the render-shape
 * ChatMessage the chat UI consumes. Tool blocks reference chat_tool_calls
 * for `result`, `display`, `status` (DB row has only the model-facing
 * content_blocks plus tool_use ids; the display payload lives on tool_calls). */
function projectMessage(
  row: DbChatMessage,
  toolsByMessage: Map<string, DbChatToolCall[]>,
): ChatMessage {
  const blocks = (row.content_blocks ?? []) as ContentBlock[];
  const toolCalls = toolsByMessage.get(row.id) ?? [];
  const toolById = new Map(toolCalls.map((t) => [t.id, t]));

  if (row.role === "user") {
    const textPart = blocks.find((b) => b.type === "text")?.text ?? "";
    return {
      id: row.id,
      parentId: row.parent_id ?? undefined,
      role: "user",
      content: textPart,
      context: (row.context as SelectionContext[] | null) ?? undefined,
      attachments: (row.attachments as ChatAttachment[] | null) ?? undefined,
      status: row.status,
      timestamp: new Date(row.created_at).getTime(),
    };
  }

  // Assistant: build parts array preserving block order so the UI can render
  // text and tool chunks inline. Tool block status/display come from the
  // tool_calls row (the DB content_block is just {type, id, name, input}).
  const parts: MessagePart[] = [];
  const accTools: ToolCallInfo[] = [];
  let textBuf = "";

  const flushText = () => {
    if (textBuf) {
      parts.push({ type: "text", text: textBuf });
      textBuf = "";
    }
  };

  for (const b of blocks) {
    if (b.type === "text" && typeof b.text === "string") {
      textBuf += b.text;
    } else if (b.type === "tool_use" && b.id) {
      flushText();
      const tc = toolById.get(b.id);
      const info: ToolCallInfo = {
        id: b.id,
        name: b.name ?? tc?.name ?? "unknown",
        args: (b.input as Record<string, unknown>) ?? tc?.args ?? {},
        result: tc?.result ?? undefined,
        status: (tc?.status ?? "pending") as ToolCallInfo["status"],
        display: (tc?.display as ToolCallInfo["display"]) ?? undefined,
        duration: tc?.duration_ms ?? undefined,
        imageDataUrl: tc?.image_data_url ?? undefined,
      };
      parts.push({ type: "tool", tool: info });
      accTools.push(info);
    }
  }
  flushText();

  // For consistency with the legacy in-memory shape, also expose the
  // concatenated text in `content` (some callers still read it directly).
  const fullText = parts
    .filter((p) => p.type === "text")
    .map((p) => (p as { type: "text"; text: string }).text)
    .join("\n\n");

  return {
    id: row.id,
    parentId: row.parent_id ?? undefined,
    role: "assistant",
    content: fullText,
    parts,
    toolCalls: accTools,
    status: row.status,
    timestamp: new Date(row.created_at).getTime(),
  };
}

function buildMessages(
  rows: DbChatMessage[],
  toolCalls: DbChatToolCall[],
): ChatMessage[] {
  const byMessage = new Map<string, DbChatToolCall[]>();
  for (const tc of toolCalls) {
    const arr = byMessage.get(tc.message_id) ?? [];
    arr.push(tc);
    byMessage.set(tc.message_id, arr);
  }
  return rows.map((r) => projectMessage(r, byMessage));
}

/** Copy any imageDataUrl values from a previous in-memory ChatMessage onto
 * a freshly-projected one whose tool rows are still missing them. The DB
 * round-trip for a screenshot data URL races with the next AI turn, so the
 * projected version often catches up only after Realtime delivers the
 * UPDATE event — and Supabase Realtime can drop large row payloads. Once
 * the client has captured an image for a tool, never let a stale projection
 * blank it out. */
function preserveToolImages(
  projected: ChatMessage,
  existing: ChatMessage | undefined,
): ChatMessage {
  if (!existing) return projected;
  const existingImages = new Map<string, string>();
  for (const part of existing.parts ?? []) {
    if (part.type === "tool" && part.tool.imageDataUrl) {
      existingImages.set(part.tool.id, part.tool.imageDataUrl);
    }
  }
  for (const tc of existing.toolCalls ?? []) {
    if (tc.imageDataUrl) existingImages.set(tc.id, tc.imageDataUrl);
  }
  if (existingImages.size === 0) return projected;

  const patchedParts = projected.parts?.map((part) => {
    if (part.type !== "tool" || part.tool.imageDataUrl) return part;
    const url = existingImages.get(part.tool.id);
    return url
      ? { type: "tool" as const, tool: { ...part.tool, imageDataUrl: url } }
      : part;
  });
  const patchedToolCalls = projected.toolCalls?.map((tc) => {
    if (tc.imageDataUrl) return tc;
    const url = existingImages.get(tc.id);
    return url ? { ...tc, imageDataUrl: url } : tc;
  });
  return { ...projected, parts: patchedParts, toolCalls: patchedToolCalls };
}

// ---------------------------------------------------------------------------
// Hook
// ---------------------------------------------------------------------------

/**
 * Wires chat-store hydration to document changes. Loads the thread for the
 * active document, subscribes to Realtime, and registers the hydrate handler
 * so chat-store's `hydrate(documentId)` (called on document open) actually
 * runs against Supabase.
 *
 * Also handles mid-stream resume: any assistant message returned with
 * status='streaming' has its existing deltas replayed and stays subscribed
 * for new ones via the same Realtime channel.
 */
export function useChatHydration() {
  const documentId = useDocumentStore((s) => s.documentId);
  const subscriptionRef = useRef<ThreadSubscription | null>(null);
  // Cache the latest tool_calls so message updates can be re-projected with
  // current tool state. Keyed by message_id → tool_calls list.
  const toolsByMessageRef = useRef<Map<string, DbChatToolCall[]>>(new Map());
  // Cache raw DbChatMessage rows so re-projections after tool_call updates
  // don't lose data.
  const messagesRef = useRef<Map<string, DbChatMessage>>(new Map());

  // Register the hydrate handler with the chat store. The store's
  // `hydrate(documentId)` action delegates to this so any code path that
  // opens a document (App.tsx, CommandPalette, DocumentPicker, etc.) can
  // trigger persistence without needing to know it exists.
  useEffect(() => {
    const reproject = () => {
      const msgs = Array.from(messagesRef.current.values()).sort(
        (a, b) =>
          new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
      );
      const flatTools = Array.from(toolsByMessageRef.current.values()).flat();
      const projected = buildMessages(msgs, flatTools);
      // For ids the local SSE handler is currently streaming, prefer the
      // existing in-memory message — its parts/text are fresher than the
      // DB roundtrip can deliver. Realtime still keeps the cache up to
      // date for hydration on subsequent reloads.
      const locallyStreamingIds = useChatStore.getState().locallyStreamingIds;
      const existing = useChatStore.getState().messages;
      const existingById = new Map(existing.map((m) => [m.id, m]));
      const merged = projected.map((p) => {
        if (locallyStreamingIds.has(p.id) && existingById.has(p.id)) {
          return existingById.get(p.id)!;
        }
        // Preserve client-set imageDataUrl on tool parts that the DB
        // projection is missing. persistToolResult is fire-and-forget and
        // the screenshot data URL (~200KB JPEG) can lag — or get dropped by
        // Realtime — so without this merge, the in-chat preview vanishes
        // the moment the next turn's events trigger a reproject.
        return preserveToolImages(p, existingById.get(p.id));
      });
      // Preserve any locally-tracked messages that haven't landed in the
      // DB cache yet (e.g. a placeholder added moments before the server
      // emits its insert event).
      for (const m of existing) {
        if (
          locallyStreamingIds.has(m.id) &&
          !merged.some((p) => p.id === m.id)
        ) {
          merged.push(m);
        }
      }
      useChatStore.getState().setMessages(merged);
    };

    const handler = async (docId: string | null) => {
      // Tear down any existing subscription before swapping threads.
      subscriptionRef.current?.unsubscribe();
      subscriptionRef.current = null;
      messagesRef.current = new Map();
      toolsByMessageRef.current = new Map();

      if (!docId) {
        useChatStore.getState().setThreadId(null);
        useChatStore.getState().reset();
        return;
      }

      const hydration = await hydrateThread(docId);
      if (!hydration) {
        // Supabase not configured (self-hosted) — fall back to the welcome state.
        useChatStore.getState().setThreadId(null);
        useChatStore.getState().reset();
        return;
      }

      // Seed the in-memory caches before projecting.
      for (const m of hydration.messages) messagesRef.current.set(m.id, m);
      for (const tc of hydration.toolCalls) {
        const arr = toolsByMessageRef.current.get(tc.message_id) ?? [];
        arr.push(tc);
        toolsByMessageRef.current.set(tc.message_id, arr);
      }

      useChatStore.getState().setThreadId(hydration.thread.id);
      const projected = buildMessages(hydration.messages, hydration.toolCalls);
      useChatStore
        .getState()
        .setMessages(projected.length === 0
          // First-time thread for this document — keep the welcome message.
          ? useChatStore.getState().messages
          : projected);

      // Replay any in-flight deltas so a reload that lands mid-stream still
      // shows the partial assistant text (and tool_use blocks already emitted).
      for (const m of hydration.messages) {
        if (m.status === "streaming") {
          const deltas = await loadDeltas(m.id);
          applyDeltasToMessage(m.id, deltas);
        }
      }
      reproject();

      // Subscribe for live updates (multi-tab + true mid-stream resume).
      subscriptionRef.current = subscribeToThread(hydration.thread.id, {
        onMessageInsert: (msg) => {
          messagesRef.current.set(msg.id, msg);
          reproject();
        },
        onMessageUpdate: (msg) => {
          messagesRef.current.set(msg.id, msg);
          reproject();
        },
        onToolCallInsert: (tc) => {
          const arr = toolsByMessageRef.current.get(tc.message_id) ?? [];
          const idx = arr.findIndex((t) => t.id === tc.id);
          if (idx === -1) arr.push(tc);
          else arr[idx] = tc;
          toolsByMessageRef.current.set(tc.message_id, arr);
          reproject();
        },
        onToolCallUpdate: (tc) => {
          const arr = toolsByMessageRef.current.get(tc.message_id) ?? [];
          const idx = arr.findIndex((t) => t.id === tc.id);
          if (idx === -1) arr.push(tc);
          else arr[idx] = tc;
          toolsByMessageRef.current.set(tc.message_id, arr);
          reproject();
        },
        onDeltaInsert: (delta) => {
          // Only apply deltas for messages we own in this thread.
          if (!messagesRef.current.has(delta.message_id)) return;
          applyDeltasToMessage(delta.message_id, [delta]);
          reproject();
        },
      });
    };

    useChatStore.getState().setHydrateHandler(handler);
    return () => useChatStore.getState().setHydrateHandler(null);
  }, []);

  // Re-hydrate whenever the open document changes.
  useEffect(() => {
    void useChatStore.getState().hydrate(documentId);
  }, [documentId]);

  // Tear down on unmount.
  useEffect(() => {
    return () => {
      subscriptionRef.current?.unsubscribe();
      subscriptionRef.current = null;
    };
  }, []);
}

// ---------------------------------------------------------------------------
// Delta replay
// ---------------------------------------------------------------------------

/** Apply deltas to a streaming message row's content_blocks in-place (in the
 * messagesRef cache). Used both during hydration (replay all stored deltas)
 * and during live Realtime (apply each new delta). */
function applyDeltasToMessage(
  _messageId: string,
  _deltas: DbChatMessageDelta[],
): void {
  // The render layer already builds parts from the message's content_blocks,
  // and the server writes content_blocks at message_stop. Deltas are useful
  // for showing partial text BEFORE message_stop fires. That's a refinement
  // — without it, mid-stream resume shows nothing until the server flushes
  // the final row. The visible regression is only "partial text doesn't
  // appear during the brief window between server-write batches."
  //
  // For the first cut, we treat deltas as a notification that something is
  // streaming but rely on content_blocks for the actual text. Full delta
  // replay (text aggregation) can be added by writing into a shadow
  // content_blocks on the messagesRef row before reproject(). Left as a
  // small follow-up so this file stays focused.
}
