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

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  context?: SelectionContext[];
  toolCalls?: ToolCallInfo[];
  /** Chronological sequence of text and tool-call chunks. Used for inline rendering. */
  parts?: MessagePart[];
  /** Images attached by the user (e.g. viewport screenshots). */
  attachments?: ChatAttachment[];
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

export interface ChatState {
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
  /** AbortController for the in-flight stream, registered by useChatHandler.
   * `requestCancel` calls `.abort()` on this so the fetch is interrupted
   * immediately instead of waiting for the current turn to finish. */
  _abortController: AbortController | null;

  // Visibility
  setOpen: (open: boolean) => void;
  toggleOpen: () => void;

  // Message actions
  addUserMessage: (
    content: string,
    context?: SelectionContext[],
    attachments?: ChatAttachment[],
  ) => void;
  addAssistantMessage: (content: string, toolCalls?: ToolCallInfo[]) => void;
  updateLastAssistant: (content: string, toolCalls?: ToolCallInfo[], parts?: MessagePart[]) => void;

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
  timestamp: Date.now(),
};

export const useChatStore = create<ChatState>((set, get) => ({
  messages: [WELCOME_MESSAGE],
  open: true,
  streaming: false,
  error: null,
  cancelRequested: false,
  anonUsage: loadAnonUsage(),
  usageError: null,
  _sendHandler: null,
  _abortController: null,

  setOpen: (open) => set({ open }),

  toggleOpen: () => set((s) => ({ open: !s.open })),

  addUserMessage: (content, context, attachments) =>
    set((s) => ({
      messages: [
        ...s.messages,
        {
          id: makeId(),
          role: "user",
          content,
          context,
          attachments: attachments && attachments.length > 0 ? attachments : undefined,
          timestamp: Date.now(),
        },
      ],
    })),

  addAssistantMessage: (content, toolCalls) =>
    set((s) => ({
      messages: [
        ...s.messages,
        {
          id: makeId(),
          role: "assistant",
          content,
          toolCalls,
          timestamp: Date.now(),
        },
      ],
    })),

  updateLastAssistant: (content, toolCalls, parts) =>
    set((s) => {
      const last = s.messages[s.messages.length - 1];
      if (!last || last.role !== "assistant") return s;
      const updated: ChatMessage = { ...last, content };
      if (toolCalls !== undefined) updated.toolCalls = toolCalls;
      if (parts !== undefined) updated.parts = parts;
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

  incAnonUsage: () => set((s) => {
    const used = s.anonUsage.used + 1;
    persistAnonUsage(used);
    return { anonUsage: { used, limit: s.anonUsage.limit } };
  }),
  setUsageError: (err) => set({ usageError: err }),

  clearThread: () => set({ messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }], streaming: false, error: null, cancelRequested: false, usageError: null }),

  reset: () => set({ messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }], open: true, streaming: false, error: null, cancelRequested: false, usageError: null }),
}));
