import { create } from "zustand";
import type { ExecutionDisplay } from "../commands/types.js";

// ---------------------------------------------------------------------------
// Anon usage persistence
// ---------------------------------------------------------------------------

const ANON_USAGE_KEY = "vcad:chat-anon-usage";
const ANON_FREE_LIMIT = 3;

/** Load anon message counter from localStorage. Resets if 24h have elapsed. */
function loadAnonUsage(): { used: number; limit: number } {
  if (typeof localStorage === "undefined") return { used: 0, limit: ANON_FREE_LIMIT };
  try {
    const raw = localStorage.getItem(ANON_USAGE_KEY);
    if (!raw) return { used: 0, limit: ANON_FREE_LIMIT };
    const parsed = JSON.parse(raw) as { used: number; firstAt: number };
    const age = Date.now() - (parsed.firstAt ?? 0);
    if (age > 24 * 60 * 60 * 1000) return { used: 0, limit: ANON_FREE_LIMIT };
    return { used: parsed.used ?? 0, limit: ANON_FREE_LIMIT };
  } catch {
    return { used: 0, limit: ANON_FREE_LIMIT };
  }
}

function persistAnonUsage(used: number): void {
  if (typeof localStorage === "undefined") return;
  try {
    // Preserve firstAt if it exists so the 24h window is anchored to first usage.
    const existing = localStorage.getItem(ANON_USAGE_KEY);
    const firstAt = existing
      ? ((JSON.parse(existing) as { firstAt?: number }).firstAt ?? Date.now())
      : Date.now();
    localStorage.setItem(ANON_USAGE_KEY, JSON.stringify({ used, firstAt }));
  } catch {
    /* localStorage quota/privacy — non-fatal */
  }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SelectionContext {
  partId: string;
  partName: string;
  geometryType: "part" | "face" | "edge" | "vertex";
  faceIndex?: number;
  dimensions?: Record<string, number>;
}

export interface ToolCallInfo {
  id: string;
  name: string;
  args: Record<string, unknown>;
  result?: unknown;
  status: "pending" | "success" | "error";
  display?: ExecutionDisplay;
  duration?: number;
  /** Optional data URL (e.g. data:image/jpeg;base64,...) for tools that
   * produce an image — rendered as a thumbnail in the tool chip. */
  imageDataUrl?: string;
}

/** A chronological chunk in an assistant message — text or a tool call. */
export type MessagePart =
  | { type: "text"; text: string }
  | { type: "tool"; tool: ToolCallInfo };

/** An image attached to a user message — captured viewport, pasted image,
 * uploaded file. Stored as a data URL on the message so the UI can preview
 * it and the chat handler can decode it into an Anthropic image content block
 * at send time. */
export interface ChatAttachment {
  id: string;
  /** Full `data:<media-type>;base64,<data>` URL. */
  dataUrl: string;
  mediaType: string;
  /** Optional filename for display. */
  filename?: string;
}

export type ChatMessageStatus =
  | "pending"
  | "streaming"
  | "complete"
  | "interrupted"
  | "error";

export interface ChatMessage {
  id: string;
  /** Parent message id in the thread DAG. null for the root message. The
   * schema supports branching (multiple children per parent); the UI renders
   * a linear path along `parent_id` chains today. */
  parentId?: string | null;
  role: "user" | "assistant";
  content: string;
  context?: SelectionContext[];
  toolCalls?: ToolCallInfo[];
  /** Chronological sequence of text and tool-call chunks. Used for inline rendering. */
  parts?: MessagePart[];
  /** Images attached by the user (e.g. viewport screenshots). */
  attachments?: ChatAttachment[];
  /** Lifecycle status. `streaming` means the server is actively writing this
   * row; `interrupted` means the stream was cut off (server died or user
   * aborted) and can't be resumed. */
  status?: ChatMessageStatus;
  timestamp: number;
}

// ---------------------------------------------------------------------------
// State shape
// ---------------------------------------------------------------------------

/** Payload from a 429 rate-limit response. */
export interface ChatUsageError {
  kind: "anon_limit" | "monthly_limit";
  message: string;
  usage?: number;
  limit?: number;
  resetsAt?: string;
}

/** Implementation of sendMessage, registered by the app-layer chat handler at
 * mount time. The store can't depend on `chat-api.ts` directly (layering), so
 * the streaming logic registers itself here and the store just delegates. */
export type SendMessageHandler = (
  content: string,
  context: SelectionContext[],
  attachments?: ChatAttachment[],
) => void;

/** Implementation of hydrate, registered by the app-layer hydration hook.
 * Wired separately because the store doesn't depend on @vcad/auth (the
 * persistence layer lives there). */
export type HydrateHandler = (documentId: string | null) => Promise<void>;

export interface ChatState {
  /** Active thread id from chat_threads. null when no document is open or
   * Supabase isn't configured. */
  threadId: string | null;
  /** True while hydrate() is in flight — UI shows a soft loading state and
   * disables sends. */
  hydrating: boolean;
  messages: ChatMessage[];
  open: boolean;
  streaming: boolean;
  error: string | null;
  /** True while the user has requested cancellation of the current response. */
  cancelRequested: boolean;
  /** Anon message count (from localStorage) with rolling 24h window. */
  anonUsage: { used: number; limit: number };
  /** Server-rejected rate limit for the most recent send attempt. */
  usageError: ChatUsageError | null;
  /** Implementation of sendMessage, populated at mount by useChatHandler. */
  _sendHandler: SendMessageHandler | null;
  /** Implementation of hydrate, populated at mount by useChatHydration. */
  _hydrateHandler: HydrateHandler | null;
  /** AbortController for the in-flight stream, registered by useChatHandler.
   * `requestCancel` calls `.abort()` on this so the fetch is interrupted
   * immediately instead of waiting for the current turn to finish. */
  _abortController: AbortController | null;
  /** Message ids whose content is being streamed by THIS tab right now.
   * Realtime updates for these ids are ignored by useChatHydration so the
   * local SSE-driven render isn't overwritten by the slower DB roundtrip. */
  locallyStreamingIds: Set<string>;

  // Visibility
  setOpen: (open: boolean) => void;
  toggleOpen: () => void;

  // Thread
  setThreadId: (id: string | null) => void;
  hydrate: (documentId: string | null) => Promise<void>;
  setHydrateHandler: (fn: HydrateHandler | null) => void;
  setHydrating: (hydrating: boolean) => void;
  /** Replace the entire message array with a fresh hydration snapshot.
   * Used by the persistence layer after fetching the thread or a Realtime
   * full-resync. */
  setMessages: (messages: ChatMessage[]) => void;
  /** Apply a single Realtime upsert: insert a new message or replace one
   * by id. Used when the server inserts/updates a row. */
  upsertMessage: (msg: ChatMessage) => void;

  // Message actions (legacy — kept for the streaming UI's optimistic updates;
  // the persistence layer reconciles via upsertMessage + setMessages).
  /** Append a user message. Pass `id` to use a pre-generated id so the
   * local row matches the persisted one. */
  addUserMessage: (
    content: string,
    context?: SelectionContext[],
    attachments?: ChatAttachment[],
    id?: string,
  ) => void;
  /** Append an assistant message. Pass `id` to use a pre-generated id so
   * the local placeholder matches the persisted row id (Realtime upserts
   * key on id). */
  addAssistantMessage: (
    content: string,
    toolCalls?: ToolCallInfo[],
    id?: string,
  ) => void;
  updateLastAssistant: (
    content: string,
    toolCalls?: ToolCallInfo[],
    parts?: MessagePart[],
    status?: ChatMessageStatus,
  ) => void;

  // Send (delegated to registered handler — call from any UI component)
  sendMessage: (
    content: string,
    context: SelectionContext[],
    attachments?: ChatAttachment[],
  ) => void;
  setSendHandler: (fn: SendMessageHandler | null) => void;

  // Status
  setStreaming: (streaming: boolean) => void;
  setError: (error: string | null) => void;

  // Cancellation
  requestCancel: () => void;
  clearCancel: () => void;
  setAbortController: (ac: AbortController | null) => void;

  // Local streaming tracking
  markLocallyStreaming: (id: string) => void;
  unmarkLocallyStreaming: (id: string) => void;

  // Usage tracking
  incAnonUsage: () => void;
  setUsageError: (err: ChatUsageError | null) => void;

  // Thread management
  clearThread: () => void;
  reset: () => void;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

const WELCOME_MESSAGE: ChatMessage = {
  id: "welcome",
  role: "assistant",
  content: "Hi! I'm your vcad assistant. I can create and modify parts, answer questions about CAD, and help you design. Try asking me to add a shape, or select something in the viewport and ask me about it.",
  status: "complete",
  timestamp: Date.now(),
};

export const useChatStore = create<ChatState>((set, get) => ({
  threadId: null,
  hydrating: false,
  messages: [WELCOME_MESSAGE],
  open: true,
  streaming: false,
  error: null,
  cancelRequested: false,
  anonUsage: loadAnonUsage(),
  usageError: null,
  _sendHandler: null,
  _hydrateHandler: null,
  _abortController: null,
  locallyStreamingIds: new Set(),

  setOpen: (open) => set({ open }),

  toggleOpen: () => set((s) => ({ open: !s.open })),

  setThreadId: (id) => set({ threadId: id }),

  hydrate: async (documentId) => {
    const handler = get()._hydrateHandler;
    if (!handler) {
      // Hydration handler isn't mounted yet (Supabase not configured, or App
      // root hasn't mounted useChatHydration). Reset to the welcome state so
      // the UI is at least consistent.
      set({
        threadId: null,
        messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }],
        hydrating: false,
      });
      return;
    }
    set({ hydrating: true });
    try {
      await handler(documentId);
    } finally {
      set({ hydrating: false });
    }
  },

  setHydrateHandler: (fn) => set({ _hydrateHandler: fn }),

  setHydrating: (hydrating) => set({ hydrating }),

  setMessages: (messages) => set({ messages }),

  upsertMessage: (msg) =>
    set((s) => {
      const idx = s.messages.findIndex((m) => m.id === msg.id);
      if (idx === -1) return { messages: [...s.messages, msg] };
      const next = [...s.messages];
      next[idx] = msg;
      return { messages: next };
    }),

  addUserMessage: (content, context, attachments, id) =>
    set((s) => ({
      messages: [
        ...s.messages,
        {
          id: id ?? makeId(),
          role: "user",
          content,
          context,
          attachments: attachments && attachments.length > 0 ? attachments : undefined,
          status: "complete",
          timestamp: Date.now(),
        },
      ],
    })),

  addAssistantMessage: (content, toolCalls, id) =>
    set((s) => ({
      messages: [
        ...s.messages,
        {
          id: id ?? makeId(),
          role: "assistant",
          content,
          toolCalls,
          status: "streaming",
          timestamp: Date.now(),
        },
      ],
    })),

  updateLastAssistant: (content, toolCalls, parts, status) =>
    set((s) => {
      const last = s.messages[s.messages.length - 1];
      if (!last || last.role !== "assistant") return s;
      const updated: ChatMessage = { ...last, content };
      if (toolCalls !== undefined) updated.toolCalls = toolCalls;
      if (parts !== undefined) updated.parts = parts;
      if (status !== undefined) updated.status = status;
      return { messages: [...s.messages.slice(0, -1), updated] };
    }),

  sendMessage: (content, context, attachments) => {
    const handler = get()._sendHandler;
    if (!handler) {
      // Handler not yet registered — useChatHandler is mounted at App root,
      // so this only fires if something dispatches before App mounts.
      console.warn("[chat-store] sendMessage called before handler registered");
      return;
    }
    handler(content, context, attachments);
  },

  setSendHandler: (fn) => set({ _sendHandler: fn }),

  setStreaming: (streaming) => set({ streaming }),

  setError: (error) => set({ error }),

  requestCancel: () => {
    get()._abortController?.abort();
    set({ cancelRequested: true });
  },
  clearCancel: () => set({ cancelRequested: false }),
  setAbortController: (ac) => set({ _abortController: ac }),

  markLocallyStreaming: (id) =>
    set((s) => {
      const next = new Set(s.locallyStreamingIds);
      next.add(id);
      return { locallyStreamingIds: next };
    }),
  unmarkLocallyStreaming: (id) =>
    set((s) => {
      if (!s.locallyStreamingIds.has(id)) return s;
      const next = new Set(s.locallyStreamingIds);
      next.delete(id);
      return { locallyStreamingIds: next };
    }),

  incAnonUsage: () => set((s) => {
    const used = s.anonUsage.used + 1;
    persistAnonUsage(used);
    return { anonUsage: { used, limit: s.anonUsage.limit } };
  }),
  setUsageError: (err) => set({ usageError: err }),

  clearThread: () => set({
    messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }],
    streaming: false,
    error: null,
    cancelRequested: false,
    usageError: null,
  }),

  reset: () => set({
    threadId: null,
    messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }],
    open: true,
    streaming: false,
    error: null,
    cancelRequested: false,
    usageError: null,
  }),
}));
