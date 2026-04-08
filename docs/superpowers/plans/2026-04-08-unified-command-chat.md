# Unified Command + Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an AI chat sidebar with context pills, wire S-key to the existing command palette, and lay the foundation for a unified tool API that connects in-app chat to the CAD engine.

**Architecture:** The chat sidebar is a new right-panel React component backed by a Zustand store. It calls a new `/api/chat` Vercel API route that uses AI SDK `streamText` with tool definitions mapped to document store actions. The existing command palette (Cmd+K) gains S-key as an additional trigger and an "Ask AI" escalation path. Context pills are driven by the existing `useUiStore` selection state.

**Tech Stack:** React 19, Zustand 5, Vercel AI SDK (`ai` package v6+), AI Gateway (OIDC auth, no provider API keys), Radix UI, Tailwind CSS, existing `@vcad/core` command registry.

---

### Task 1: Install AI SDK Dependencies

**Files:**
- Modify: `package.json` (root)
- Modify: `packages/app/package.json`

- [ ] **Step 1: Install AI SDK packages**

Run:
```bash
npm install ai -w @vcad/app
```

Note: No provider-specific package needed — AI Gateway handles provider routing via OIDC auth. The `ai` package routes `"provider/model"` strings through the gateway automatically.

- [ ] **Step 2: Verify install succeeded**

Run: `npm ls ai`
Expected: Package listed without errors.

- [ ] **Step 3: Commit**

```bash
git add package.json package-lock.json packages/app/package.json
git commit -m "feat: add AI SDK and Anthropic provider dependencies"
```

---

### Task 2: Create Chat Zustand Store

**Files:**
- Create: `packages/core/src/stores/chat-store.ts`
- Modify: `packages/core/src/index.ts` (re-export)

- [ ] **Step 1: Write test for chat store**

Create: `packages/core/src/__tests__/chat-store.test.ts`

```ts
import { describe, it, expect, beforeEach } from "vitest";
import { useChatStore } from "../stores/chat-store.js";

describe("chatStore", () => {
  beforeEach(() => {
    useChatStore.getState().reset();
  });

  it("starts with empty messages and open=false", () => {
    const state = useChatStore.getState();
    expect(state.messages).toEqual([]);
    expect(state.open).toBe(false);
  });

  it("addUserMessage appends a user message with context", () => {
    useChatStore.getState().addUserMessage("fillet 3mm", [
      { partId: "part-1", partName: "Box_1", geometryType: "part" },
    ]);
    const msgs = useChatStore.getState().messages;
    expect(msgs).toHaveLength(1);
    expect(msgs[0].role).toBe("user");
    expect(msgs[0].content).toBe("fillet 3mm");
    expect(msgs[0].context).toHaveLength(1);
    expect(msgs[0].context![0].partName).toBe("Box_1");
  });

  it("addAssistantMessage appends an assistant message", () => {
    useChatStore.getState().addAssistantMessage("Done — applied 3mm fillet.");
    const msgs = useChatStore.getState().messages;
    expect(msgs).toHaveLength(1);
    expect(msgs[0].role).toBe("assistant");
  });

  it("clearThread resets messages but preserves open state", () => {
    useChatStore.getState().setOpen(true);
    useChatStore.getState().addUserMessage("hello", []);
    useChatStore.getState().clearThread();
    const state = useChatStore.getState();
    expect(state.messages).toEqual([]);
    expect(state.open).toBe(true);
  });

  it("setOpen toggles the sidebar", () => {
    useChatStore.getState().setOpen(true);
    expect(useChatStore.getState().open).toBe(true);
    useChatStore.getState().setOpen(false);
    expect(useChatStore.getState().open).toBe(false);
  });

  it("setStreaming tracks AI response streaming state", () => {
    useChatStore.getState().setStreaming(true);
    expect(useChatStore.getState().streaming).toBe(true);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run packages/core/src/__tests__/chat-store.test.ts`
Expected: FAIL — module `../stores/chat-store.js` not found.

- [ ] **Step 3: Implement the chat store**

Create: `packages/core/src/stores/chat-store.ts`

```ts
import { create } from "zustand";

/** Context attached to a user message from viewport selection. */
export interface SelectionContext {
  partId: string;
  partName: string;
  geometryType: "part" | "face" | "edge" | "vertex";
  faceIndex?: number;
  dimensions?: { x: number; y: number; z: number };
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  context?: SelectionContext[];
  toolCalls?: ToolCallInfo[];
  timestamp: number;
}

export interface ToolCallInfo {
  id: string;
  name: string;
  args: Record<string, unknown>;
  result?: string;
  status: "pending" | "success" | "error";
}

export interface ChatState {
  messages: ChatMessage[];
  open: boolean;
  streaming: boolean;
  error: string | null;

  setOpen: (open: boolean) => void;
  toggleOpen: () => void;
  addUserMessage: (content: string, context: SelectionContext[]) => void;
  addAssistantMessage: (content: string, toolCalls?: ToolCallInfo[]) => void;
  updateLastAssistant: (content: string, toolCalls?: ToolCallInfo[]) => void;
  setStreaming: (streaming: boolean) => void;
  setError: (error: string | null) => void;
  clearThread: () => void;
  reset: () => void;
}

let nextId = 0;
function genId(): string {
  return `msg-${++nextId}-${Date.now()}`;
}

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  open: false,
  streaming: false,
  error: null,

  setOpen: (open) => set({ open }),
  toggleOpen: () => set((s) => ({ open: !s.open })),

  addUserMessage: (content, context) =>
    set((s) => ({
      messages: [
        ...s.messages,
        { id: genId(), role: "user", content, context, timestamp: Date.now() },
      ],
    })),

  addAssistantMessage: (content, toolCalls) =>
    set((s) => ({
      messages: [
        ...s.messages,
        { id: genId(), role: "assistant", content, toolCalls, timestamp: Date.now() },
      ],
    })),

  updateLastAssistant: (content, toolCalls) =>
    set((s) => {
      const msgs = [...s.messages];
      const last = msgs[msgs.length - 1];
      if (last?.role === "assistant") {
        msgs[msgs.length - 1] = { ...last, content, toolCalls: toolCalls ?? last.toolCalls };
      }
      return { messages: msgs };
    }),

  setStreaming: (streaming) => set({ streaming }),
  setError: (error) => set({ error }),
  clearThread: () => set({ messages: [] }),
  reset: () => set({ messages: [], open: false, streaming: false, error: null }),
}));
```

- [ ] **Step 4: Export from core index**

Add to `packages/core/src/index.ts`:

```ts
export { useChatStore } from "./stores/chat-store.js";
export type { ChatMessage, SelectionContext, ToolCallInfo, ChatState } from "./stores/chat-store.js";
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npx vitest run packages/core/src/__tests__/chat-store.test.ts`
Expected: All 5 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/stores/chat-store.ts packages/core/src/__tests__/chat-store.test.ts packages/core/src/index.ts
git commit -m "feat: add chat Zustand store with message and context pill types"
```

---

### Task 3: Create Chat Sidebar Component

**Files:**
- Create: `packages/app/src/components/ChatSidebar.tsx`
- Modify: `packages/app/src/App.tsx`

- [ ] **Step 1: Create the ChatSidebar component**

Create: `packages/app/src/components/ChatSidebar.tsx`

```tsx
import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { PaperPlaneTilt } from "@phosphor-icons/react/dist/ssr/PaperPlaneTilt";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { cn } from "@/lib/utils";
import { useChatStore, useUiStore, useDocumentStore } from "@vcad/core";
import type { SelectionContext, ChatMessage, ToolCallInfo } from "@vcad/core";

/** Build context pills from current viewport selection. */
function useSelectionContext(): SelectionContext[] {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const partIndex = useDocumentStore((s) => s.partIndex);

  if (selectedPartIds.size === 0) return [];

  const contexts: SelectionContext[] = [];
  for (const id of selectedPartIds) {
    const part = partIndex.get(id);
    if (part) {
      contexts.push({
        partId: id,
        partName: part.name,
        geometryType: "part",
      });
    }
  }
  return contexts;
}

function ContextPill({ ctx, onRemove }: { ctx: SelectionContext; onRemove: () => void }) {
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-accent/30 bg-accent/10 px-2 py-0.5 text-[10px] text-accent">
      <span className="text-[8px]">&#x2B21;</span>
      {ctx.partName}
      <button onClick={onRemove} className="ml-0.5 opacity-50 hover:opacity-100">
        <X size={8} />
      </button>
    </span>
  );
}

function ToolCallCard({ tool }: { tool: ToolCallInfo }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="rounded border border-border bg-bg p-1.5 text-[10px]">
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex w-full items-center gap-1"
      >
        {tool.status === "success" && <span className="text-success">&#x2713;</span>}
        {tool.status === "error" && <span className="text-danger">&#x2717;</span>}
        {tool.status === "pending" && <SpinnerGap size={10} className="animate-spin text-text-muted" />}
        <span className="text-text-muted">{tool.name}</span>
        <CaretRight
          size={8}
          className={cn("ml-auto text-text-muted transition-transform", expanded && "rotate-90")}
        />
      </button>
      {expanded && (
        <pre className="mt-1 overflow-x-auto text-[9px] text-text-muted">
          {JSON.stringify(tool.args, null, 2)}
        </pre>
      )}
    </div>
  );
}

function MessageBubble({ msg }: { msg: ChatMessage }) {
  const isUser = msg.role === "user";

  return (
    <div className="mb-3">
      <div className="mb-1 flex items-center gap-1">
        <div
          className={cn(
            "flex h-4 w-4 items-center justify-center rounded-full text-[8px]",
            isUser ? "bg-surface-hover text-text" : "bg-purple-600 text-white",
          )}
        >
          {isUser ? "Y" : "v"}
        </div>
        <span className="text-[10px] text-text-muted">{isUser ? "You" : "vcad"}</span>
      </div>
      <div className="pl-5">
        {/* Context pills on user messages */}
        {isUser && msg.context && msg.context.length > 0 && (
          <div className="mb-1 flex flex-wrap gap-1">
            {msg.context.map((ctx) => (
              <span
                key={ctx.partId}
                className="inline-flex items-center gap-1 rounded-full border border-accent/30 bg-accent/10 px-2 py-0.5 text-[10px] text-accent"
              >
                <span className="text-[8px]">&#x2B21;</span>
                {ctx.partName}
              </span>
            ))}
          </div>
        )}
        {/* Tool call cards */}
        {msg.toolCalls?.map((tool) => (
          <div key={tool.id} className="mb-1.5">
            <ToolCallCard tool={tool} />
          </div>
        ))}
        {/* Message text */}
        <div className="text-[11px] leading-relaxed text-text">
          {msg.content}
        </div>
      </div>
    </div>
  );
}

export function ChatSidebar() {
  const open = useChatStore((s) => s.open);
  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const setOpen = useChatStore((s) => s.setOpen);
  const addUserMessage = useChatStore((s) => s.addUserMessage);
  const clearThread = useChatStore((s) => s.clearThread);

  const [input, setInput] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const selectionContext = useSelectionContext();
  const [attachedContext, setAttachedContext] = useState<SelectionContext[]>([]);

  // Sync selection context into attached pills
  useEffect(() => {
    if (selectionContext.length > 0) {
      setAttachedContext(selectionContext);
    }
  }, [selectionContext]);

  // Auto-scroll on new messages
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages]);

  // Focus input when sidebar opens
  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  const handleSend = useCallback(() => {
    const text = input.trim();
    if (!text || streaming) return;

    addUserMessage(text, attachedContext);
    setInput("");
    setAttachedContext([]);

    // Dispatch event for the chat handler to pick up
    window.dispatchEvent(
      new CustomEvent("vcad:chat-send", {
        detail: { content: text, context: attachedContext },
      }),
    );
  }, [input, streaming, attachedContext, addUserMessage]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const removeContext = (partId: string) => {
    setAttachedContext((prev) => prev.filter((c) => c.partId !== partId));
  };

  if (!open) return null;

  const placeholder = attachedContext.length > 0
    ? `Ask about ${attachedContext[0].partName}...`
    : "Ask anything...";

  return (
    <div className="pointer-events-auto fixed right-0 top-0 z-20 flex h-full w-[260px] flex-col border-l border-border bg-card">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-border px-3 py-2">
        <span className="text-[11px] font-semibold text-text">Chat</span>
        <div className="flex items-center gap-2">
          <button onClick={clearThread} className="text-[10px] text-text-muted hover:text-text">
            New
          </button>
          <button onClick={() => setOpen(false)} className="text-text-muted hover:text-text">
            <X size={12} />
          </button>
        </div>
      </div>

      {/* Messages */}
      <div ref={scrollRef} className="flex-1 overflow-y-auto p-3">
        {messages.length === 0 && (
          <div className="flex h-full items-center justify-center">
            <p className="text-center text-[11px] text-text-muted">
              Ask questions or give commands.<br />
              Select geometry for context.
            </p>
          </div>
        )}
        {messages.map((msg) => (
          <MessageBubble key={msg.id} msg={msg} />
        ))}
        {streaming && (
          <div className="flex items-center gap-2 pl-5 text-[11px] text-text-muted">
            <SpinnerGap size={12} className="animate-spin" />
            Thinking...
          </div>
        )}
      </div>

      {/* Input area */}
      <div className="border-t border-border p-2">
        {/* Context pills */}
        {attachedContext.length > 0 && (
          <div className="mb-1.5 flex flex-wrap gap-1">
            {attachedContext.map((ctx) => (
              <ContextPill
                key={ctx.partId}
                ctx={ctx}
                onRemove={() => removeContext(ctx.partId)}
              />
            ))}
          </div>
        )}
        <div className="flex items-end gap-1.5">
          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            rows={1}
            className="flex-1 resize-none rounded border border-border bg-bg px-2.5 py-2 text-[11px] text-text outline-none placeholder:text-text-muted/50 focus:border-accent"
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || streaming}
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded border border-border bg-bg text-text-muted hover:text-accent disabled:opacity-30"
          >
            <PaperPlaneTilt size={14} />
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Wire ChatSidebar into App.tsx**

In `packages/app/src/App.tsx`, add the lazy import near the other lazy imports (around line 34):

```ts
const ChatSidebar = lazy(() => import("@/components/ChatSidebar").then(m => ({ default: m.ChatSidebar })));
```

Add the import for `useChatStore`:

```ts
import { useChatStore } from "@vcad/core";
```

Inside the `App()` component's return JSX, after the AIPanel Suspense block (around line 645), add:

```tsx
{/* Chat sidebar */}
<Suspense fallback={null}>
  <ChatSidebar />
</Suspense>
```

- [ ] **Step 3: Build and verify no errors**

Run: `npm run build -w @vcad/core && npm run build -w @vcad/app`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add packages/app/src/components/ChatSidebar.tsx packages/app/src/App.tsx
git commit -m "feat: add chat sidebar component with context pills and message thread"
```

---

### Task 4: Add S-Key Trigger to Command Palette

**Files:**
- Modify: `packages/app/src/hooks/useKeyboardShortcuts.ts`
- Modify: `packages/core/src/commands.ts` (update Scale Mode shortcut)

- [ ] **Step 1: Check current S-key binding**

Read `packages/app/src/hooks/useKeyboardShortcuts.ts` and find what S is currently bound to. From `packages/core/src/commands.ts` line 143, S is "Scale Mode". We need to remap Scale Mode to a different shortcut and use S for the command palette.

- [ ] **Step 2: Change Scale Mode shortcut from S to Shift+S**

In `packages/core/src/commands.ts`, change the Scale Mode entry:

```ts
// Before:
shortcut: "S",
// After:
shortcut: "Shift+S",
```

- [ ] **Step 3: Add S-key to open command palette in useKeyboardShortcuts**

In `packages/app/src/hooks/useKeyboardShortcuts.ts`, find the keyboard handler and add:

```ts
// S key opens command palette (Fusion 360 muscle memory)
if (e.key === "s" && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey) {
  e.preventDefault();
  useUiStore.getState().setCommandPaletteOpen(true);
}
```

Make sure this is gated behind the same "not in text input" check that other shortcuts use.

- [ ] **Step 4: Build and verify**

Run: `npm run build -w @vcad/core && npm run build -w @vcad/app`
Expected: Build succeeds.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/commands.ts packages/app/src/hooks/useKeyboardShortcuts.ts
git commit -m "feat: bind S-key to command palette, remap Scale Mode to Shift+S"
```

---

### Task 5: Add "Ask AI" Escalation from Command Palette to Chat

**Files:**
- Modify: `packages/app/src/components/CommandPalette.tsx`

- [ ] **Step 1: Import chat store**

Add to imports in `CommandPalette.tsx`:

```ts
import { useChatStore } from "@vcad/core";
```

- [ ] **Step 2: Add escalation to chat when no command matches and Enter pressed**

In the `handleKeyDown` function (around line 507), modify the Enter case. Currently when there are no command matches and there's a query, it calls `handleAIGenerate(aiPrompt)` which generates CAD via cad0. Change this to open the chat sidebar instead:

Replace the else-if branch in the Enter handler (around line 531). The existing code calls the cad0 generator. Change it to:

```ts
// Before (existing code that calls the cad0 generator):
else if (aiPrompt) {
  handleAIGenerate(aiPrompt);
}

// After:
else if (aiPrompt) {
  // Escalate to AI chat sidebar
  const chatStore = useChatStore.getState();
  chatStore.setOpen(true);
  // Pre-fill the chat with this query
  const selContext = Array.from(selectedPartIds).map((id) => {
    const part = parts.find((p) => p.id === id);
    return part
      ? { partId: id, partName: part.name, geometryType: "part" as const }
      : null;
  }).filter(Boolean) as import("@vcad/core").SelectionContext[];
  chatStore.addUserMessage(aiPrompt, selContext);
  window.dispatchEvent(
    new CustomEvent("vcad:chat-send", {
      detail: { content: aiPrompt, context: selContext },
    }),
  );
  onOpenChange(false);
}
```

- [ ] **Step 3: Update the "AI Generate" section label**

Change the section header text from "AI Generate" to "Ask AI" (around line 709):

```tsx
// Before:
<span>AI Generate</span>
// After:
<span>Ask AI</span>
```

- [ ] **Step 4: Build and verify**

Run: `npm run build -w @vcad/app`
Expected: Build succeeds.

- [ ] **Step 5: Commit**

```bash
git add packages/app/src/components/CommandPalette.tsx
git commit -m "feat: escalate unmatched command palette queries to AI chat sidebar"
```

---

### Task 6: Create Chat API Route

**Files:**
- Create: `api/chat.ts`
- Create: `packages/app/src/lib/chat-api.ts`

- [ ] **Step 1: Create the API route**

Create: `api/chat.ts`

```ts
/**
 * Vercel API route for AI chat.
 *
 * Uses AI SDK streamText with tool definitions for CAD operations.
 * Provider is abstracted — currently Anthropic/Claude, switchable via env var.
 */

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { streamText } from "ai";

const SYSTEM_PROMPT = `You are vcad's AI assistant — a parametric CAD copilot embedded in a web-based CAD application.

You can both answer questions about CAD design and execute operations on the user's model.

Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.

When the user asks you to modify geometry:
1. Use the available tools to execute the operation
2. Briefly confirm what you did after the tool call completes
3. If a tool call fails, explain the error and suggest alternatives

When the user asks questions:
- Be concise and practical
- Reference specific parts by name when relevant
- If you need more context about their model, ask

Context pills in user messages indicate what geometry is currently selected in the viewport. Use this context to understand which parts the user is referring to.`;

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

  const { messages, context } = req.body as {
    messages: Array<{ role: "user" | "assistant"; content: string }>;
    context?: { selectedParts: Array<{ partId: string; partName: string; geometryType: string }> };
  };

  if (!messages?.length) {
    res.status(400).json({ error: "messages required" });
    return;
  }

  // Build context-aware system prompt
  let systemPrompt = SYSTEM_PROMPT;
  if (context?.selectedParts?.length) {
    const partList = context.selectedParts
      .map((p) => `- ${p.partName} (${p.geometryType}, id: ${p.partId})`)
      .join("\n");
    systemPrompt += `\n\nCurrently selected geometry:\n${partList}`;
  }

  try {
    const result = streamText({
      model: "anthropic/claude-sonnet-4.6",
      system: systemPrompt,
      messages,
      tools: {
        // Tool definitions will be expanded in Task 7
        // For now, the AI can answer questions without tools
      },
    });

    // Stream the response using AI SDK v6 toTextStreamResponse
    return result.toTextStreamResponse();
  } catch (err) {
    console.error("Chat API error:", err);
    res.status(500).json({ error: "Internal server error" });
  }
}
```

- [ ] **Step 2: Create the client-side chat API helper**

Create: `packages/app/src/lib/chat-api.ts`

```ts
/**
 * Client-side helper for calling the chat API route.
 * Uses AI SDK's streaming utilities.
 */

import type { SelectionContext } from "@vcad/core";

export interface ChatRequestMessage {
  role: "user" | "assistant";
  content: string;
}

export interface ChatStreamCallbacks {
  onText: (text: string) => void;
  onToolCall?: (toolCall: { id: string; name: string; args: Record<string, unknown> }) => void;
  onToolResult?: (toolResult: { id: string; result: string; status: "success" | "error" }) => void;
  onError: (error: string) => void;
  onFinish: () => void;
}

export async function streamChat(
  messages: ChatRequestMessage[],
  context: SelectionContext[],
  callbacks: ChatStreamCallbacks,
): Promise<void> {
  const selectedParts = context.map((c) => ({
    partId: c.partId,
    partName: c.partName,
    geometryType: c.geometryType,
  }));

  try {
    const response = await fetch("/api/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        messages,
        context: { selectedParts },
      }),
    });

    if (!response.ok) {
      const err = await response.text();
      callbacks.onError(err || `HTTP ${response.status}`);
      callbacks.onFinish();
      return;
    }

    const reader = response.body?.getReader();
    if (!reader) {
      callbacks.onError("No response body");
      callbacks.onFinish();
      return;
    }

    const decoder = new TextDecoder();
    let buffer = "";
    let fullText = "";

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });

      // Parse AI SDK data stream format: lines starting with data type prefix
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";

      for (const line of lines) {
        if (!line.trim()) continue;

        // AI SDK data stream format: "0:text" for text chunks
        if (line.startsWith("0:")) {
          const text = JSON.parse(line.slice(2)) as string;
          fullText += text;
          callbacks.onText(fullText);
        }
        // "e:" for finish
        else if (line.startsWith("e:")) {
          // Stream finished
        }
      }
    }

    callbacks.onFinish();
  } catch (err) {
    callbacks.onError(err instanceof Error ? err.message : "Stream failed");
    callbacks.onFinish();
  }
}
```

- [ ] **Step 3: Build and verify**

Run: `npm run build -w @vcad/app`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add api/chat.ts packages/app/src/lib/chat-api.ts
git commit -m "feat: add chat API route with AI SDK streaming and client helper"
```

---

### Task 7: Wire Chat Sidebar to API

**Files:**
- Create: `packages/app/src/hooks/useChatHandler.ts`
- Modify: `packages/app/src/App.tsx` (add the hook)

- [ ] **Step 1: Create the chat handler hook**

Create: `packages/app/src/hooks/useChatHandler.ts`

```ts
/**
 * Hook that listens for chat events and calls the API.
 * Runs at the App level — handles the vcad:chat-send custom event.
 */

import { useEffect, useCallback } from "react";
import { useChatStore } from "@vcad/core";
import type { SelectionContext, ChatMessage } from "@vcad/core";
import { streamChat } from "@/lib/chat-api";

export function useChatHandler() {
  const handleChatSend = useCallback(
    async (e: CustomEvent<{ content: string; context: SelectionContext[] }>) => {
      const { content, context } = e.detail;
      const store = useChatStore.getState();

      // Build message history for the API (last 20 messages for context window)
      const history = store.messages.slice(-20).map((msg) => ({
        role: msg.role as "user" | "assistant",
        content: msg.content,
      }));

      // Add placeholder assistant message
      store.addAssistantMessage("");
      store.setStreaming(true);
      store.setError(null);

      await streamChat(history, context, {
        onText: (text) => {
          store.updateLastAssistant(text);
        },
        onError: (error) => {
          store.setError(error);
          store.updateLastAssistant(`Error: ${error}`);
        },
        onFinish: () => {
          store.setStreaming(false);
        },
      });
    },
    [],
  );

  useEffect(() => {
    const handler = handleChatSend as EventListener;
    window.addEventListener("vcad:chat-send", handler);
    return () => window.removeEventListener("vcad:chat-send", handler);
  }, [handleChatSend]);
}
```

- [ ] **Step 2: Add the hook to App.tsx**

In `packages/app/src/App.tsx`, import and call the hook:

```ts
import { useChatHandler } from "@/hooks/useChatHandler";
```

Inside the `App()` function, after the existing hooks (around line 128):

```ts
useChatHandler();
```

- [ ] **Step 3: Build and verify**

Run: `npm run build -w @vcad/app`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add packages/app/src/hooks/useChatHandler.ts packages/app/src/App.tsx
git commit -m "feat: wire chat sidebar to API with streaming responses"
```

---

### Task 8: Add Chat Toggle to UI

**Files:**
- Modify: `packages/app/src/components/CornerIcons.tsx` (add chat toggle button)
- Modify: `packages/app/src/hooks/useKeyboardShortcuts.ts` (add Cmd+Shift+L shortcut)

- [ ] **Step 1: Read CornerIcons.tsx to understand the current layout**

Read `packages/app/src/components/CornerIcons.tsx` and find where the existing buttons are rendered (top-right area).

- [ ] **Step 2: Add a chat toggle button to CornerIcons**

Add a chat toggle button using the existing button styling pattern. Import `ChatDots` from Phosphor icons and `useChatStore` from `@vcad/core`:

```tsx
import { ChatDots } from "@phosphor-icons/react/dist/ssr/ChatDots";
import { useChatStore } from "@vcad/core";
```

Add the toggle button near the other top-right controls:

```tsx
<button
  onClick={() => useChatStore.getState().toggleOpen()}
  className={cn(
    "flex h-8 w-8 items-center justify-center rounded text-text-muted hover:text-text hover:bg-surface-hover",
    useChatStore.getState().open && "text-accent",
  )}
  title="Toggle chat (⌘⇧L)"
>
  <ChatDots size={16} />
</button>
```

- [ ] **Step 3: Add keyboard shortcut Cmd+Shift+L to toggle chat**

In `packages/app/src/hooks/useKeyboardShortcuts.ts`, add:

```ts
// Cmd+Shift+L toggles chat sidebar
if (e.key === "l" && (e.metaKey || e.ctrlKey) && e.shiftKey) {
  e.preventDefault();
  useChatStore.getState().toggleOpen();
}
```

Add the import:
```ts
import { useChatStore } from "@vcad/core";
```

- [ ] **Step 4: Build and verify**

Run: `npm run build -w @vcad/app`
Expected: Build succeeds.

- [ ] **Step 5: Commit**

```bash
git add packages/app/src/components/CornerIcons.tsx packages/app/src/hooks/useKeyboardShortcuts.ts
git commit -m "feat: add chat toggle button and Cmd+Shift+L keyboard shortcut"
```

---

### Task 9: Environment Variables and Vercel Config

**Files:**
- Modify: `vercel.json` (add chat API function config)
- Create: `.env.example` (document required env vars)

- [ ] **Step 1: Check current vercel.json for function config pattern**

Read `vercel.json` and understand the existing function routing.

- [ ] **Step 2: Add chat route to vercel.json**

Add the chat API route to the functions config, following the existing pattern for `api/mcp.ts`:

```json
{
  "functions": {
    "api/chat.ts": {
      "maxDuration": 30
    }
  }
}
```

Add a route entry if routes are used (check existing pattern).

- [ ] **Step 3: Pull OIDC token for AI Gateway auth**

AI Gateway uses OIDC auth — no provider API keys needed. Run:

```bash
vercel link        # connect to your Vercel project (if not already linked)
vercel env pull .env.local   # provisions VERCEL_OIDC_TOKEN automatically
```

The `ai` package reads `VERCEL_OIDC_TOKEN` from `.env.local` and routes requests through the AI Gateway. On Vercel deployments, OIDC tokens are auto-refreshed. For local dev, re-run `vercel env pull .env.local --yes` when the token expires (~24h).

- [ ] **Step 4: Verify .env.local is in .gitignore**

Run: `grep -q "\.env" .gitignore && echo "OK" || echo "ADD .env TO .gitignore"`

- [ ] **Step 5: Commit**

```bash
git add vercel.json .env.example
git commit -m "feat: configure chat API route for Vercel deployment"
```

---

### Task 10: End-to-End Smoke Test

**Files:** None new — this is verification only.

- [ ] **Step 1: Ensure AI Gateway OIDC token is available**

Run: `vercel env pull .env.local --yes`
This provisions `VERCEL_OIDC_TOKEN` for local AI Gateway auth. No provider API keys needed.

- [ ] **Step 2: Start the dev server**

Run: `npm run dev -w @vcad/app`

- [ ] **Step 3: Test S-key opens command palette**

Open browser to localhost. Press S. Command palette should open.

- [ ] **Step 4: Test "Ask AI" escalation**

In the command palette, type "how do I make a bracket for 3D printing?" — should see "Ask AI" at the bottom. Press Enter — chat sidebar should open on the right with the query and a streaming AI response.

- [ ] **Step 5: Test context pills**

Add a box (via command palette: type "box", Enter). Select it in the viewport. Open chat (Cmd+Shift+L). Verify a context pill appears above the chat input showing the box's name.

- [ ] **Step 6: Test chat toggle**

Click the chat button in the corner icons. Sidebar should toggle. Press Cmd+Shift+L — should toggle again.

- [ ] **Step 7: Commit integration test notes**

No code to commit — this is manual verification. If issues were found and fixed, commit those fixes.

---

### Future Tasks (Not in This Plan)

These are explicitly deferred per the spec's non-goals:

1. **Tool definitions for CAD operations** — expand `api/chat.ts` with tool definitions that map to document store actions (add_fillet, set_parameter, etc.)
2. **Chat thread persistence** — save threads to Supabase for logged-in users
3. **Free tier rate limiting** — localStorage counter for logged-out users
4. **MCP tool API redesign** — auto-generate from IR types, unify with chat tools
5. **Navigation presets** — Fusion 360 mouse mode
6. **Toolbar reorganization** — regroup tools in Fusion-familiar order
7. **Property panel removal** — inline editing in feature tree
