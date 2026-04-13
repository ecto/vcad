import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
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
import { useAuth, AuthModal } from "@vcad/auth";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Message, MessageContent, MessageResponse } from "@/components/ai-elements/message";
import {
  PromptInput,
  PromptInputHeader,
  PromptInputTextarea,
  PromptInputSubmit,
} from "@/components/ai-elements/prompt-input";
import type { PromptInputMessage } from "@/components/ai-elements/prompt-input";
import { Shimmer } from "@/components/ai-elements/shimmer";
import { VcadToolCard } from "@/components/chat/VcadToolCard";
import { CadSuggestions } from "@/components/chat/CadSuggestions";

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

function VcadMessage({ msg }: { msg: ChatMessage }) {
  const isUser = msg.role === "user";

  // Assistant messages have a parts array (text + tool chunks interleaved).
  // User messages only have content + context. Both need to render through
  // the same Message shell so AI Elements' bubble styling cascades correctly.
  const hasParts = !isUser && msg.parts && msg.parts.length > 0;
  const summary = !isUser ? summarizeToolParts(msg.parts) : null;

  return (
    <Message from={isUser ? "user" : "assistant"}>
      <MessageContent className="text-[11px]">
        {/* Context pills attached to the user bubble */}
        {isUser && msg.context && msg.context.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {msg.context.map((ctx) => (
              <span
                key={ctx.partId}
                className="inline-flex items-center gap-1 rounded border border-accent/30 bg-accent/10 px-1.5 py-0.5 text-[9px] text-accent"
              >
                {ctx.partName}
              </span>
            ))}
          </div>
        )}

        {isUser ? (
          msg.content && <span className="whitespace-pre-wrap">{msg.content}</span>
        ) : hasParts ? (
          <>
            {msg.parts!.map((part, i) =>
              part.type === "text" ? (
                part.text.trim() ? (
                  <MessageResponse key={`text-${i}`}>{part.text}</MessageResponse>
                ) : null
              ) : (
                <VcadToolCard key={part.tool.id} call={part.tool} />
              ),
            )}
            {summary && (
              <p className="text-[9px] italic text-text-muted">{summary}</p>
            )}
          </>
        ) : (
          /* Legacy assistant message without parts array */
          <>
            {msg.toolCalls?.map((call) => (
              <VcadToolCard key={call.id} call={call} />
            ))}
            {msg.content && <MessageResponse>{msg.content}</MessageResponse>}
          </>
        )}
      </MessageContent>
    </Message>
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
  const messages = useChatStore((s) => s.messages);
  const streaming = useChatStore((s) => s.streaming);
  const setOpen = useChatStore((s) => s.setOpen);
  const clearThread = useChatStore((s) => s.clearThread);
  const anonUsage = useChatStore((s) => s.anonUsage);
  const usageError = useChatStore((s) => s.usageError);
  const setUsageError = useChatStore((s) => s.setUsageError);
  const { user } = useAuth();

  const [activeTab, setActiveTab] = useState<SidebarTab>("chat");
  const [selectionContext, removeContextPart] = useSelectionContext();
  const [showAuthModal, setShowAuthModal] = useState(false);

  // When the user hits the anon limit, open the sign-in modal automatically.
  useEffect(() => {
    if (usageError?.kind === "anon_limit") {
      setShowAuthModal(true);
    }
  }, [usageError]);

  const sendMessage = useCallback(
    (content: string) => {
      const trimmed = content.trim();
      if (!trimmed || streaming) return;
      window.dispatchEvent(
        new CustomEvent("vcad:chat-send", {
          detail: {
            content: trimmed,
            context: selectionContext.length > 0 ? selectionContext : [],
          },
        }),
      );
    },
    [selectionContext, streaming],
  );

  const handlePromptSubmit = useCallback(
    (message: PromptInputMessage) => {
      sendMessage(message.text);
    },
    [sendMessage],
  );

  // Suggestion chips dispatch directly without going through the textarea.
  const handleSuggestionPick = useCallback(
    (text: string) => {
      sendMessage(text);
    },
    [sendMessage],
  );

  const firstCtx = selectionContext[0];
  const placeholder =
    selectionContext.length === 1 && firstCtx
      ? `Ask about ${firstCtx.partName}...`
      : selectionContext.length > 1
        ? `Ask about ${selectionContext.length} selected parts...`
        : "Ask anything...";

  // Map vcad's streaming flag to PromptInputSubmit's status enum.
  const submitStatus: "submitted" | "streaming" | "ready" = streaming ? "streaming" : "ready";

  // True when the very last visible chunk is a tool call still running — that's
  // when the Shimmer "thinking" line is most informative (vs. just streaming text).
  const lastMsg = messages[messages.length - 1];
  const lastPart = lastMsg?.parts?.[lastMsg.parts.length - 1];
  const showShimmer =
    streaming &&
    (lastPart?.type === "tool" || (!lastPart && lastMsg?.role === "assistant"));

  return (
    <div
      className={cn(
        "ai-elements-scope",
        "flex h-full w-full flex-col",
        "bg-surface",
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
      {activeTab === "chat" && (
        <>
          <Conversation className="flex-1 min-h-0">
            <ConversationContent className="gap-4 p-3">
              {messages.length === 0 && (
                <ConversationEmptyState
                  title="Ask vcad anything"
                  description="Request changes to your model, ask questions about geometry, or pick a suggestion below."
                />
              )}
              {messages.map((msg) => (
                <VcadMessage key={msg.id} msg={msg} />
              ))}
              {showShimmer && (
                <Shimmer className="px-1 text-[11px]">Thinking...</Shimmer>
              )}
            </ConversationContent>
            <ConversationScrollButton />
          </Conversation>

          {/* Usage banners — borderless, distinguished by tinted bg only */}
          {usageError?.kind === "monthly_limit" && (
            <div className="shrink-0 bg-danger/10 px-4 py-2 text-[10px] text-danger">
              <div className="mb-0.5 font-semibold">Monthly chat limit reached</div>
              <div className="text-text-muted">{usageError.message}</div>
            </div>
          )}
          {usageError?.kind === "anon_limit" && (
            <div className="shrink-0 bg-accent/10 px-4 py-2 text-[10px] text-text">
              <div className="mb-0.5 font-semibold text-accent">Free chat limit reached</div>
              <div className="text-text-muted">{usageError.message}</div>
              <button
                onClick={() => setShowAuthModal(true)}
                className="mt-1 rounded bg-accent px-2 py-0.5 text-[9px] text-white hover:bg-accent/90"
              >
                Sign in to continue
              </button>
            </div>
          )}
          {!user && anonUsage.used > 0 && !usageError && (
            <div className="shrink-0 px-4 py-1 text-center text-[9px] text-text-muted">
              {Math.min(anonUsage.used, anonUsage.limit)}/{anonUsage.limit} free chat messages used
            </div>
          )}

          {/* Compose dock — suggestions + input grouped in one padded surface,
              no dividing borders. The input's own border is the only line. */}
          <div className="shrink-0 space-y-2.5 px-3 pb-3 pt-1">
            <CadSuggestions
              selection={selectionContext}
              onPick={handleSuggestionPick}
            />

            <PromptInput onSubmit={handlePromptSubmit}>
              {selectionContext.length > 0 && (
                <PromptInputHeader>
                  {selectionContext.map((ctx) => (
                    <span
                      key={ctx.partId}
                      className="inline-flex items-center gap-1 rounded-full border border-accent/30 bg-accent/10 py-0.5 pl-2 pr-1 text-[9px] text-accent"
                    >
                      {ctx.partName}
                      <button
                        onClick={() => removeContextPart(ctx.partId)}
                        className="flex h-3 w-3 items-center justify-center rounded-full hover:bg-accent/20 hover:text-accent/80 transition-colors"
                        aria-label={`Remove ${ctx.partName}`}
                      >
                        <X size={8} />
                      </button>
                    </span>
                  ))}
                </PromptInputHeader>
              )}
              <PromptInputTextarea
                placeholder={placeholder}
                disabled={streaming}
                className="text-[11px]"
              />
              <PromptInputSubmit
                status={submitStatus}
                onStop={() => useChatStore.getState().requestCancel()}
              />
            </PromptInput>
          </div>
        </>
      )}

      {/* Auth modal — opens automatically when anon limit is hit, or on demand */}
      <AuthModal
        open={showAuthModal}
        onOpenChange={(v) => {
          setShowAuthModal(v);
          // Clear the rate-limit error if the user dismisses the modal so it
          // doesn't immediately re-open on the next state change.
          if (!v && usageError?.kind === "anon_limit") {
            setUsageError(null);
          }
        }}
        feature="ai"
      />
    </div>
  );
}
