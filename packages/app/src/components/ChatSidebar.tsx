import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { PaperPlaneTilt } from "@phosphor-icons/react/dist/ssr/PaperPlaneTilt";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { cn } from "@/lib/utils";
import {
  useChatStore,
  useUiStore,
  useDocumentStore,
  useEngineStore,
  parseVcadFile,
  documentToLoon,
} from "@vcad/core";
import type { SelectionContext, ChatMessage, MessagePart } from "@vcad/core";
import { ToolCallCard } from "@/components/chat/ToolCallCard";

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
// Message row
// ---------------------------------------------------------------------------

/**
 * Tally successful tool parts into a short natural-language summary.
 * Returns null if there are fewer than 2 successful tool calls.
 */
function summarizeToolParts(parts: MessagePart[] | undefined): string | null {
  if (!parts) return null;
  const tallies = {
    created: 0,
    cut: 0,
    joined: 0,
    modified: 0,
    moved: 0,
    finished: 0,
    deleted: 0,
    colored: 0,
  };
  let successToolCount = 0;
  for (const p of parts) {
    if (p.type !== "tool") continue;
    const tool = p.tool;
    if (tool.status !== "success") continue;
    successToolCount++;
    const argType = (tool.args.type as string) ?? "";
    if (tool.name === "create") {
      if (
        [
          "cube",
          "cylinder",
          "sphere",
          "cone",
          "extrude",
          "revolve",
          "sweep",
          "loft",
          "sketch_2d",
          "text_2d",
        ].includes(argType)
      ) {
        tallies.created++;
      } else if (argType === "difference") {
        tallies.cut++;
      } else if (argType === "union" || argType === "intersection") {
        tallies.joined++;
      } else if (["translate", "rotate", "scale"].includes(argType)) {
        tallies.moved++;
      } else if (
        ["fillet", "chamfer", "shell", "linear_pattern", "circular_pattern"].includes(argType)
      ) {
        tallies.finished++;
      }
    } else if (tool.name === "update") {
      tallies.modified++;
    } else if (tool.name === "delete") {
      tallies.deleted++;
    } else if (tool.name === "set_material") {
      tallies.colored++;
    }
  }
  if (successToolCount < 2) return null;
  const nonZero = Object.entries(tallies).filter(([, v]) => v > 0);
  if (nonZero.length === 0) return null;
  return nonZero.map(([k, v]) => `${v} ${k}`).join(" · ");
}

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

      {/* Chronological parts (assistant messages with parts) */}
      {!isUser && msg.parts && msg.parts.length > 0 ? (
        <div className="pl-5 space-y-1.5">
          {msg.parts.map((part, i) =>
            part.type === "text" ? (
              part.text.trim() ? (
                <p key={`text-${i}`} className="text-[11px] text-text leading-relaxed whitespace-pre-wrap">
                  {part.text}
                </p>
              ) : null
            ) : (
              <ToolCallCard key={part.tool.id} call={part.tool} />
            )
          )}
          {(() => {
            const summary = summarizeToolParts(msg.parts);
            return summary ? (
              <p className="text-[9px] text-text-muted italic">{summary}</p>
            ) : null;
          })()}
        </div>
      ) : (
        <>
          {/* Fallback: legacy tool calls + content for messages without parts */}
          {!isUser && msg.toolCalls && msg.toolCalls.length > 0 && (
            <div className="pl-5 mb-1.5">
              {msg.toolCalls.map((call) => (
                <ToolCallCard key={call.id} call={call} />
              ))}
            </div>
          )}
          {msg.content && (
            <p className="pl-5 text-[11px] text-text leading-relaxed whitespace-pre-wrap">
              {msg.content}
            </p>
          )}
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// SourcePanel — live-synced loon source view of the current document
// ---------------------------------------------------------------------------

function SourcePanel() {
  const document = useDocumentStore((s) => s.document);
  const streaming = useChatStore((s) => s.streaming);
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const [localSource, setLocalSource] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [isDirty, setIsDirty] = useState(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const syncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Sync from the document, but skip while user is editing, AI is streaming, or gizmo is being dragged
  useEffect(() => {
    if (isDirty || streaming || isDraggingGizmo) return;
    // Debounce to avoid re-entrant WASM calls during rapid document updates
    if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
    syncTimerRef.current = setTimeout(() => {
      try {
        setLocalSource(documentToLoon(document));
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }, 150);
    return () => {
      if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
    };
  }, [document, isDirty, streaming, isDraggingGizmo]);

  const evalAndLoad = useCallback((source: string) => {
    const engine = useEngineStore.getState().engine;
    if (!engine) return;
    try {
      const evalLoon = (s: string) => {
        const doc = engine.evalVcadSource(s);
        if (!doc) throw new Error("Loon evaluation not supported");
        return JSON.stringify(doc);
      };
      const vcadFile = parseVcadFile(source, evalLoon);
      useDocumentStore.getState().loadDocument(vcadFile);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      const value = e.target.value;
      setLocalSource(value);
      setIsDirty(true);
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        evalAndLoad(value);
        setIsDirty(false);
      }, 300);
    },
    [evalAndLoad],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        if (debounceRef.current) clearTimeout(debounceRef.current);
        evalAndLoad(localSource);
      }
      if (e.key === "Tab") {
        e.preventDefault();
        const ta = e.currentTarget;
        const start = ta.selectionStart;
        const end = ta.selectionEnd;
        const val = ta.value;
        const next = val.substring(0, start) + "  " + val.substring(end);
        setLocalSource(next);
        requestAnimationFrame(() => {
          ta.selectionStart = ta.selectionEnd = start + 2;
        });
      }
    },
    [localSource, evalAndLoad],
  );

  return (
    <div className="flex h-full flex-col">
      <textarea
        value={localSource}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        spellCheck={false}
        className="flex-1 resize-none bg-bg p-3 font-mono text-[10px] leading-relaxed text-text outline-none"
        placeholder={"; Write loon source here\n[cube 20.0 20.0 20.0]"}
      />
      {error && (
        <div className="border-t border-danger/30 bg-danger/10 px-3 py-2 text-[10px] text-danger shrink-0">
          {error}
        </div>
      )}
      <div className="border-t border-border px-3 py-1 text-[9px] text-text-muted shrink-0">
        {isDirty ? "Editing (will eval on pause)" : "Synced with document"} · ⌘⏎ to eval
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// ChatSidebar
// ---------------------------------------------------------------------------

type SidebarTab = "chat" | "source";

export function ChatSidebar() {
  const open = useChatStore((s) => s.open);
  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const setOpen = useChatStore((s) => s.setOpen);
  const clearThread = useChatStore((s) => s.clearThread);

  const [activeTab, setActiveTab] = useState<SidebarTab>("chat");
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
      {/* Header with tabs */}
      <div className="flex items-center gap-1 px-2 py-1.5 border-b border-border shrink-0">
        <button
          onClick={() => setActiveTab("chat")}
          className={cn(
            "px-2 py-1 text-[10px] font-semibold rounded transition-colors",
            activeTab === "chat"
              ? "bg-accent/10 text-accent"
              : "text-text-muted hover:text-text hover:bg-hover"
          )}
        >
          Chat
        </button>
        <button
          onClick={() => setActiveTab("source")}
          className={cn(
            "px-2 py-1 text-[10px] font-semibold rounded transition-colors",
            activeTab === "source"
              ? "bg-accent/10 text-accent"
              : "text-text-muted hover:text-text hover:bg-hover"
          )}
        >
          Source
        </button>
        <div className="flex-1" />
        {activeTab === "chat" && (
          <button
            onClick={clearThread}
            className="flex items-center gap-1 px-1.5 py-0.5 text-[10px] text-text-muted hover:text-text hover:bg-hover rounded transition-colors"
            title="New thread"
          >
            <Plus size={11} />
            New
          </button>
        )}
        <button
          onClick={() => setOpen(false)}
          className="flex h-5 w-5 items-center justify-center text-text-muted hover:text-text hover:bg-hover rounded transition-colors"
          title="Close"
        >
          <X size={13} />
        </button>
      </div>

      {/* Source tab content */}
      {activeTab === "source" && <SourcePanel />}

      {/* Chat tab content — only rendered when chat tab active */}
      {activeTab === "chat" && <>

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
      </>}
    </div>
  );
}
