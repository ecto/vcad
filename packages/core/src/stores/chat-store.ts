import { create } from "zustand";
import type { ExecutionDisplay } from "../commands/types.js";

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
}

/** A chronological chunk in an assistant message — text or a tool call. */
export type MessagePart =
  | { type: "text"; text: string }
  | { type: "tool"; tool: ToolCallInfo };

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  context?: SelectionContext[];
  toolCalls?: ToolCallInfo[];
  /** Chronological sequence of text and tool-call chunks. Used for inline rendering. */
  parts?: MessagePart[];
  timestamp: number;
}

// ---------------------------------------------------------------------------
// State shape
// ---------------------------------------------------------------------------

export interface ChatState {
  messages: ChatMessage[];
  open: boolean;
  streaming: boolean;
  error: string | null;

  // Visibility
  setOpen: (open: boolean) => void;
  toggleOpen: () => void;

  // Message actions
  addUserMessage: (content: string, context?: SelectionContext[]) => void;
  addAssistantMessage: (content: string, toolCalls?: ToolCallInfo[]) => void;
  updateLastAssistant: (content: string, toolCalls?: ToolCallInfo[], parts?: MessagePart[]) => void;

  // Status
  setStreaming: (streaming: boolean) => void;
  setError: (error: string | null) => void;

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

export const useChatStore = create<ChatState>((set) => ({
  messages: [WELCOME_MESSAGE],
  open: true,
  streaming: false,
  error: null,

  setOpen: (open) => set({ open }),

  toggleOpen: () => set((s) => ({ open: !s.open })),

  addUserMessage: (content, context) =>
    set((s) => ({
      messages: [
        ...s.messages,
        {
          id: makeId(),
          role: "user",
          content,
          context,
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

  setStreaming: (streaming) => set({ streaming }),

  setError: (error) => set({ error }),

  clearThread: () => set({ messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }], streaming: false, error: null }),

  reset: () => set({ messages: [{ ...WELCOME_MESSAGE, timestamp: Date.now() }], open: true, streaming: false, error: null }),
}));
