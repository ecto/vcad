import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { PaperPlaneTilt } from "@phosphor-icons/react/dist/ssr/PaperPlaneTilt";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { cn } from "@/lib/utils";
import {
  useChatStore,
  useUiStore,
  useDocumentStore,
} from "@vcad/core";
import type { SelectionContext, ChatMessage, ToolCallInfo } from "@vcad/core";

// ---------------------------------------------------------------------------
// Hook: build SelectionContext[] from current selection
// ---------------------------------------------------------------------------

function useSelectionContext(): [SelectionContext[], (partId: string) => void] {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const partIndex = useDocumentStore((s) => s.partIndex);

  const contexts: SelectionContext[] = [];
  for (const partId of selectedPartIds) {
    const part = partIndex.get(partId);
    if (part) {
      contexts.push({
        partId,
        partName: part.name,
        geometryType: "part",
      });
    }
  }

  const removeContext = useCallback(
    (partId: string) => {
      const next = new Set(selectedPartIds);
      next.delete(partId);
      useUiStore.getState().selectMultiple([...next]);
    },
    [selectedPartIds]
  );

  return [contexts, removeContext];
}

// ---------------------------------------------------------------------------
// Tool call card
// ---------------------------------------------------------------------------

function ToolCallCard({ call }: { call: ToolCallInfo }) {
  const [expanded, setExpanded] = useState(false);

  const statusIcon =
    call.status === "success" ? (
      <Check size={10} className="text-success shrink-0" />
    ) : call.status === "error" ? (
      <XCircle size={10} className="text-error shrink-0" />
    ) : (
      <SpinnerGap size={10} className="animate-spin text-text-muted shrink-0" />
    );

  return (
    <div className="mt-1 border border-border bg-bg rounded text-[10px]">
      <button
        onClick={() => setExpanded((e) => !e)}
        className="flex w-full items-center gap-1.5 px-2 py-1 text-left hover:bg-hover transition-colors"
      >
        {statusIcon}
        <span className="font-mono text-text-muted truncate flex-1">{call.name}</span>
        <CaretRight
          size={10}
          className={cn(
            "text-text-muted transition-transform shrink-0",
            expanded && "rotate-90"
          )}
        />
      </button>
      {expanded && (
        <div className="px-2 pb-2 border-t border-border">
          <pre className="mt-1 text-[9px] text-text-muted whitespace-pre-wrap break-all font-mono leading-relaxed">
            {JSON.stringify(call.args, null, 2)}
          </pre>
          {call.result !== undefined && (
            <>
              <div className="mt-1 text-[9px] text-text-muted font-medium">Result:</div>
              <pre className="text-[9px] text-text-muted whitespace-pre-wrap break-all font-mono leading-relaxed">
                {JSON.stringify(call.result, null, 2)}
              </pre>
            </>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Message row
// ---------------------------------------------------------------------------

function MessageRow({ msg }: { msg: ChatMessage }) {
  const isUser = msg.role === "user";

  return (
    <div className="px-3 py-2">
      <div className="flex items-center gap-1.5 mb-1">
        {/* Avatar */}
        <div
          className={cn(
            "flex h-4 w-4 items-center justify-center rounded text-[9px] font-bold shrink-0",
            isUser ? "bg-bg-elevated text-text" : "bg-purple-600 text-white"
          )}
        >
          {isUser ? "Y" : "v"}
        </div>
        <span className="text-[10px] font-medium text-text">
          {isUser ? "You" : "vcad"}
        </span>
      </div>

      {/* Context pills (user messages) */}
      {isUser && msg.context && msg.context.length > 0 && (
        <div className="flex flex-wrap gap-1 mb-1.5 pl-5">
          {msg.context.map((ctx) => (
            <span
              key={ctx.partId}
              className="inline-flex items-center gap-1 px-1.5 py-0.5 bg-accent/10 border border-accent/20 rounded text-[9px] text-accent"
            >
              {ctx.partName}
            </span>
          ))}
        </div>
      )}

      {/* Tool calls (assistant messages) */}
      {!isUser && msg.toolCalls && msg.toolCalls.length > 0 && (
        <div className="pl-5 mb-1.5">
          {msg.toolCalls.map((call) => (
            <ToolCallCard key={call.id} call={call} />
          ))}
        </div>
      )}

      {/* Message text */}
      {msg.content && (
        <p className="pl-5 text-[11px] text-text leading-relaxed whitespace-pre-wrap">
          {msg.content}
        </p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// ChatSidebar
// ---------------------------------------------------------------------------

export function ChatSidebar() {
  const open = useChatStore((s) => s.open);
  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const setOpen = useChatStore((s) => s.setOpen);
  const clearThread = useChatStore((s) => s.clearThread);

  const [input, setInput] = useState("");
  const [selectionContext, removeContextPart] = useSelectionContext();

  const threadRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Scroll to bottom when messages change or streaming starts/stops
  useEffect(() => {
    if (threadRef.current) {
      threadRef.current.scrollTop = threadRef.current.scrollHeight;
    }
  }, [messages, streaming]);

  const handleSend = useCallback(() => {
    const content = input.trim();
    if (!content || streaming) return;

    const context: SelectionContext[] = selectionContext.length > 0 ? selectionContext : [];

    window.dispatchEvent(
      new CustomEvent("vcad:chat-send", {
        detail: { content, context },
      })
    );

    setInput("");
  }, [input, selectionContext, streaming]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend]
  );

  if (!open) return null;

  const firstCtx = selectionContext[0];
  const placeholder =
    selectionContext.length === 1 && firstCtx
      ? `Ask about ${firstCtx.partName}...`
      : selectionContext.length > 1
      ? `Ask about ${selectionContext.length} selected parts...`
      : "Ask anything...";

  return (
    <div
      className={cn(
        "fixed right-0 top-0 z-30 flex h-full w-[260px] flex-col",
        "bg-card border-l border-border"
      )}
    >
      {/* Header */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
        <span className="text-[11px] font-semibold text-text flex-1">Chat</span>
        <button
          onClick={clearThread}
          className="flex items-center gap-1 px-1.5 py-0.5 text-[10px] text-text-muted hover:text-text hover:bg-hover rounded transition-colors"
          title="New thread"
        >
          <Plus size={11} />
          New
        </button>
        <button
          onClick={() => setOpen(false)}
          className="flex h-5 w-5 items-center justify-center text-text-muted hover:text-text hover:bg-hover rounded transition-colors"
          title="Close"
        >
          <X size={13} />
        </button>
      </div>

      {/* Message thread */}
      <div
        ref={threadRef}
        className="flex-1 overflow-y-auto"
      >
        {messages.length === 0 && (
          <div className="flex h-full items-center justify-center p-6">
            <p className="text-center text-[11px] text-text-muted leading-relaxed">
              Ask questions about your model, request changes, or get design suggestions.
            </p>
          </div>
        )}
        {messages.map((msg) => (
          <MessageRow key={msg.id} msg={msg} />
        ))}

        {/* Streaming indicator */}
        {streaming && (
          <div className="flex items-center gap-2 px-3 py-2">
            <SpinnerGap size={12} className="animate-spin text-text-muted" />
            <span className="text-[10px] text-text-muted">Thinking...</span>
          </div>
        )}
      </div>

      {/* Input area */}
      <div className="shrink-0 border-t border-border">
        {/* Context pills */}
        {selectionContext.length > 0 && (
          <div className="flex flex-wrap gap-1 px-2 pt-2">
            {selectionContext.map((ctx) => (
              <span
                key={ctx.partId}
                className="inline-flex items-center gap-1 px-1.5 py-0.5 bg-accent/10 border border-accent/20 rounded text-[9px] text-accent"
              >
                {ctx.partName}
                <button
                  onClick={() => removeContextPart(ctx.partId)}
                  className="hover:text-accent/70 transition-colors"
                  aria-label={`Remove ${ctx.partName}`}
                >
                  <X size={9} />
                </button>
              </span>
            ))}
          </div>
        )}

        <div className="flex items-end gap-1 p-2">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            rows={1}
            className={cn(
              "flex-1 resize-none bg-bg border border-border rounded px-2 py-1.5",
              "text-[11px] text-text placeholder:text-text-muted/50",
              "focus:outline-none focus:border-accent",
              "min-h-[32px] max-h-[120px] leading-relaxed",
            )}
            style={{ height: "auto" }}
            onInput={(e) => {
              const el = e.currentTarget;
              el.style.height = "auto";
              el.style.height = `${Math.min(el.scrollHeight, 120)}px`;
            }}
            disabled={streaming}
          />
          <button
            onClick={handleSend}
            disabled={!input.trim() || streaming}
            className={cn(
              "flex h-8 w-8 shrink-0 items-center justify-center rounded",
              "bg-accent text-white",
              "hover:bg-accent/90 transition-colors",
              "disabled:opacity-40 disabled:cursor-not-allowed"
            )}
            title="Send (Enter)"
          >
            <PaperPlaneTilt size={14} />
          </button>
        </div>
      </div>
    </div>
  );
}
