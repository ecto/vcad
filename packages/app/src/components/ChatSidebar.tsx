import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Camera } from "@phosphor-icons/react/dist/ssr/Camera";
import { cn } from "@/lib/utils";
import { Tooltip } from "@/components/ui/tooltip";
import {
  useChatStore,
  useUiStore,
  useDocumentStore,
  useEngineStore,
  parseVcadFile,
  documentToLoonChecked,
  formatTokens,
} from "@vcad/core";
import type {
  SelectionContext,
  ChatMessage,
  ChatAttachment,
  MessagePart,
} from "@vcad/core";
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
  PromptInputButton,
  PromptInputFooter,
  PromptInputHeader,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  usePromptInputAttachments,
} from "@/components/ai-elements/prompt-input";
import type { PromptInputMessage } from "@/components/ai-elements/prompt-input";
import { Shimmer } from "@/components/ai-elements/shimmer";
import { VcadToolCard } from "@/components/chat/VcadToolCard";
import { CadSuggestions } from "@/components/chat/CadSuggestions";
import { UpgradeModal } from "@/components/UpgradeModal";
import { captureViewportAsFile, downscaleImageDataUrl } from "@/lib/ai-screenshot";

// ---------------------------------------------------------------------------
// Attach-viewport button — grabs the current 3D viewport canvas as a JPEG
// and adds it to the PromptInput's attachments. The user can aim the camera
// themselves before clicking, so we just capture whatever's on screen.
// ---------------------------------------------------------------------------

function AttachViewportButton() {
  const attachments = usePromptInputAttachments();
  const [capturing, setCapturing] = useState(false);

  const handleClick = useCallback(async () => {
    if (capturing) return;
    setCapturing(true);
    try {
      const file = await captureViewportAsFile();
      if (file) attachments.add([file]);
    } finally {
      setCapturing(false);
    }
  }, [attachments, capturing]);

  return (
    <PromptInputButton
      type="button"
      onClick={handleClick}
      disabled={capturing}
      tooltip="Attach viewport screenshot"
      aria-label="Attach viewport screenshot"
    >
      <Camera size={14} />
    </PromptInputButton>
  );
}

// Bridge that lets callers outside the PromptInput (the chat sidebar's
// whole-pane drop zone, or an image dropped on the 3D viewport) push files
// into the PromptInput's attachment list. Drains chat-store.pendingAttachments
// on mount (covers the viewport case, where the sidebar is lazy-mounted after
// the drop) and subscribes for subsequent pushes.
// Must render inside a <PromptInput> to access the local attachments context.
function ChatAttachmentBridge() {
  const attachments = usePromptInputAttachments();
  useEffect(() => {
    // Drain anything queued before this component mounted.
    const queued = useChatStore.getState().consumePendingAttachments();
    if (queued.length > 0) attachments.add(queued);

    // Then stay subscribed for any future queued files while the sidebar is open.
    const unsub = useChatStore.subscribe((s, prev) => {
      if (s.pendingAttachments !== prev.pendingAttachments && s.pendingAttachments.length > 0) {
        const files = useChatStore.getState().consumePendingAttachments();
        if (files.length > 0) attachments.add(files);
      }
    });
    return unsub;
  }, [attachments]);
  return null;
}

// Render the current attachment list as a thumbnail strip above the textarea.
// Users can click a thumbnail to remove it.
function AttachmentPreviewStrip() {
  const attachments = usePromptInputAttachments();
  if (attachments.files.length === 0) return null;
  return (
    <PromptInputHeader>
      {attachments.files.map((f) => (
        <div key={f.id} className="relative">
          {f.url && (
            <img
              src={f.url}
              alt={f.filename ?? "attachment"}
              className="h-12 w-12 rounded border border-border object-cover"
            />
          )}
          <button
            type="button"
            onClick={() => attachments.remove(f.id)}
            aria-label="Remove attachment"
            className="absolute -right-1 -top-1 flex h-3.5 w-3.5 items-center justify-center rounded-full bg-bg border border-border text-text-muted hover:text-text hover:bg-hover transition-colors"
          >
            <X size={8} />
          </button>
        </div>
      ))}
    </PromptInputHeader>
  );
}

// ---------------------------------------------------------------------------
// Hook: build SelectionContext[] from current selection
// ---------------------------------------------------------------------------

function useSelectionContext(): [SelectionContext[], (partId: string) => void] {
  const selection = useUiStore((s) => s.selection);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const partIndex = useDocumentStore((s) => s.partIndex);

  // Walk the full selection union — surface sub-feature picks (face / edge
  // / vertex) so the AI can act on exactly what the user has selected.
  // The historical part-only path still works because part items emit
  // geometryType: "part".
  const contexts: SelectionContext[] = [];
  for (const item of selection) {
    if (item.kind === "part") {
      const part = partIndex.get(item.id);
      if (part) {
        contexts.push({
          partId: item.id,
          partName: part.name,
          geometryType: "part",
        });
      }
    } else if (item.kind === "face") {
      const part = partIndex.get(item.partId);
      if (part) {
        contexts.push({
          partId: item.partId,
          partName: part.name,
          geometryType: "face",
          faceIndex: item.faceIndex,
        });
      }
    } else if (item.kind === "edge") {
      const part = partIndex.get(item.partId);
      if (part) {
        contexts.push({
          partId: item.partId,
          partName: part.name,
          geometryType: "edge",
          dimensions: { edgeId: item.edgeId },
        });
      }
    } else if (item.kind === "vertex") {
      const part = partIndex.get(item.partId);
      if (part) {
        contexts.push({
          partId: item.partId,
          partName: part.name,
          geometryType: "vertex",
          dimensions: { vertexId: item.vertexId },
        });
      }
    }
    // segment / constraint — sketch-mode entities, not surfaced to chat
    // here since the AI's CAD tools don't operate on those yet.
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

function VcadMessage({ msg, userName }: { msg: ChatMessage; userName: string }) {
  const isUser = msg.role === "user";

  // Assistant messages have a parts array (text + tool chunks interleaved).
  // User messages only have content + context. Both need to render through
  // the same Message shell so turn styling stays consistent.
  const hasParts = !isUser && msg.parts && msg.parts.length > 0;
  const summary = !isUser ? summarizeToolParts(msg.parts) : null;
  // Show a subtle badge when an assistant turn was cut off (server died, tab
  // closed mid-stream). Distinct from the user-initiated "[Stopped]" marker
  // which is a text part — this surfaces the persisted DB status.
  const isInterrupted = !isUser && msg.status === "interrupted";
  const isErrored = !isUser && msg.status === "error";

  // IRC-style role prefix. Brand pink for the user, muted for vcad — gives
  // each turn a scannable "who's talking" label without bubbles or alignment
  // tricks that fight the rest of the monokai/terminal UI.
  const roleLabel = isUser ? userName : "vcad";
  const roleClass = isUser ? "text-brand" : "text-text-muted";

  return (
    <Message from={isUser ? "user" : "assistant"}>
      <div
        className={cn(
          "text-[9px] uppercase tracking-wider select-none",
          roleClass,
        )}
      >
        {roleLabel}
      </div>
      <MessageContent className="text-[11px]">
        {/* Context pills attached to the user bubble */}
        {isUser && msg.context && msg.context.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {msg.context.map((ctx) => (
              <span
                key={ctx.partId}
                className="inline-flex items-center gap-1 rounded-sm border border-border bg-fill px-1.5 py-0.5 text-[10px] font-mono text-text-muted"
              >
                {ctx.partName}
              </span>
            ))}
          </div>
        )}

        {/* Attachment thumbnails on the user bubble */}
        {isUser && msg.attachments && msg.attachments.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {msg.attachments.map((a) => (
              <img
                key={a.id}
                src={a.dataUrl}
                alt={a.filename ?? "attachment"}
                className="max-h-32 rounded border border-border"
              />
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
            {(isInterrupted || isErrored) && (
              <p className="text-[9px] italic text-text-muted">
                {isInterrupted
                  ? "— interrupted before completing —"
                  : "— turn errored out —"}
              </p>
            )}
          </>
        ) : (
          /* Legacy assistant message without parts array */
          <>
            {msg.toolCalls?.map((call) => (
              <VcadToolCard key={call.id} call={call} />
            ))}
            {msg.content && <MessageResponse>{msg.content}</MessageResponse>}
            {(isInterrupted || isErrored) && (
              <p className="text-[9px] italic text-text-muted">
                {isInterrupted
                  ? "— interrupted before completing —"
                  : "— turn errored out —"}
              </p>
            )}
          </>
        )}
      </MessageContent>
    </Message>
  );
}

// ---------------------------------------------------------------------------
// AnonTrialBar — token-budget progress bar shown above the composer for
// signed-out users. Mirrors the FooterUsageMeter visual language so the
// upgrade nudge feels consistent across the app.
// ---------------------------------------------------------------------------

function AnonTrialBar({ used, limit }: { used: number; limit: number }) {
  const frac = limit > 0 ? Math.min(1, used / limit) : 0;
  const pct = frac * 100;
  const severity = frac >= 0.95 ? "critical" : frac >= 0.75 ? "warn" : "ok";
  const barColor =
    severity === "critical"
      ? "bg-danger"
      : severity === "warn"
        ? "bg-amber-500"
        : "bg-brand";
  const labelColor =
    severity === "critical"
      ? "text-danger"
      : severity === "warn"
        ? "text-amber-500"
        : "text-text-muted";
  return (
    <div
      className="shrink-0 flex items-center gap-2 px-3 py-1.5 text-[9px]"
      role="status"
      aria-label={`Free trial: ${formatTokens(used)} of ${formatTokens(limit)} tokens used`}
    >
      <span className={cn("uppercase tracking-wider", labelColor)}>
        free trial
      </span>
      <div
        className="h-1 flex-1 overflow-hidden bg-border/40"
        aria-hidden
      >
        <div
          className={cn("h-full transition-all duration-500 ease-out", barColor)}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="tabular-nums text-text-muted">
        {formatTokens(Math.min(used, limit))}
        <span className="text-text-muted/60">/{formatTokens(limit)}</span>
      </span>
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
  const [unsupportedVariants, setUnsupportedVariants] = useState<string[]>([]);
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
        const { source, unsupported } = documentToLoonChecked(document);
        setLocalSource(source);
        setUnsupportedVariants(unsupported);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        setUnsupportedVariants([]);
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
      {unsupportedVariants.length > 0 && (
        <div className="border-t border-warning/30 bg-warning/10 px-3 py-2 text-[10px] text-warning shrink-0">
          <strong>Loon export warning:</strong> This document contains{" "}
          {unsupportedVariants.join(", ")} which cannot be preserved in loon
          format. Saving as .loon will lose this content.
        </div>
      )}
      {error && (
        <div className="border-t border-danger/30 bg-danger/10 px-3 py-2 text-[10px] text-danger shrink-0">
          {error}
        </div>
      )}
      <div className="border-t border-border/40 px-3 py-1 text-[9px] text-text-muted shrink-0">
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
  const { user, isAuthenticated } = useAuth();

  // Derive a short display name for the IRC-style role prefix. Prefer the
  // user's first name, then the email prefix, then fall back to "you" for
  // anonymous sessions.
  const userName = (() => {
    const full =
      user?.user_metadata?.full_name || user?.user_metadata?.name;
    if (full) return String(full).split(" ")[0]!.toLowerCase();
    if (user?.email) return user.email.split("@")[0]!.toLowerCase();
    return "you";
  })();

  const [activeTab, setActiveTab] = useState<SidebarTab>("chat");
  const [selectionContext, removeContextPart] = useSelectionContext();
  const [showAuthModal, setShowAuthModal] = useState(false);
  const [showUpgradeModal, setShowUpgradeModal] = useState(false);
  const [isDraggingImage, setIsDraggingImage] = useState(false);
  const dragDepthRef = useRef(0);
  const inputWrapRef = useRef<HTMLDivElement>(null);

  // Whole-pane image drop. PromptInput already handles drops on its own form,
  // so we forward only drops that land outside the input wrap (messages area,
  // header, etc.) by pushing them onto chat-store.pendingAttachments, which
  // the ChatAttachmentBridge inside the PromptInput subscribes to.
  const handleDragEnter = useCallback((e: React.DragEvent<HTMLDivElement>) => {
    if (!e.dataTransfer?.types?.includes("Files")) return;
    dragDepthRef.current += 1;
    setIsDraggingImage(true);
  }, []);

  const handleDragLeave = useCallback(() => {
    dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
    if (dragDepthRef.current === 0) setIsDraggingImage(false);
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent<HTMLDivElement>) => {
    if (e.dataTransfer?.types?.includes("Files")) {
      e.preventDefault();
    }
  }, []);

  const handleDrop = useCallback((e: React.DragEvent<HTMLDivElement>) => {
    dragDepthRef.current = 0;
    setIsDraggingImage(false);

    // Drops on the PromptInput form are handled by the form's own native
    // listener — don't double-add. Still stop propagation so App's
    // viewport-level drop handler doesn't try to parse the image as a .vcad.
    const target = e.target as Node | null;
    if (target && inputWrapRef.current?.contains(target)) {
      e.stopPropagation();
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    const files = [...(e.dataTransfer?.files ?? [])].filter((f) =>
      f.type.startsWith("image/"),
    );
    if (files.length === 0) return;

    useChatStore.getState().queuePendingAttachments(files);
  }, []);

  // External callers (e.g. the welcome overlay's "Build with AI" action)
  // can dispatch `vcad:focus-chat-input` to jump cursor to the textarea.
  useEffect(() => {
    const handler = () => {
      // Defer a frame so the sidebar has mounted if it just opened.
      requestAnimationFrame(() => {
        const textarea = inputWrapRef.current?.querySelector("textarea");
        if (textarea) {
          textarea.focus();
          setActiveTab("chat");
        }
      });
    };
    window.addEventListener("vcad:focus-chat-input", handler);
    return () => window.removeEventListener("vcad:focus-chat-input", handler);
  }, []);

  // Route each kind of usage error to the right modal: anon → sign in,
  // monthly → upgrade plan. Guard the anon path against authenticated users
  // — receiving `anon_limit` while signed-in means the request was treated
  // as anonymous on the server (typically a stale token), and popping the
  // sign-in modal at that point would be both confusing and useless.
  useEffect(() => {
    if (usageError?.kind === "anon_limit" && !isAuthenticated) {
      setShowAuthModal(true);
    } else if (usageError?.kind === "monthly_limit") {
      setShowUpgradeModal(true);
    }
  }, [usageError, isAuthenticated]);

  const sendMessage = useCallback(
    (content: string, attachments?: ChatAttachment[]) => {
      const trimmed = content.trim();
      // Allow attachment-only sends (e.g. "here's a screenshot, what do you
      // think?" with an empty text field). Require at least text OR one image.
      if (!trimmed && (!attachments || attachments.length === 0)) return;
      if (streaming) return;
      useChatStore.getState().sendMessage(
        trimmed,
        selectionContext.length > 0 ? selectionContext : [],
        attachments,
      );
    },
    [selectionContext, streaming],
  );

  const handlePromptSubmit = useCallback(
    (message: PromptInputMessage) => {
      // PromptInput converts any attached blob: URLs to data URLs before
      // firing onSubmit, so message.files[i].url is already a `data:...`
      // string we can forward straight to the chat handler.
      void (async () => {
        const attachments: ChatAttachment[] = [];
        for (const f of message.files) {
          if (!f.url || !f.url.startsWith("data:")) continue;
          // Downscale to keep us under Anthropic's image size limit; large
          // source PNGs/photos blow past 5MB and the API rejects the whole
          // turn with "Could not process image".
          const scaled = await downscaleImageDataUrl(f.url);
          attachments.push({
            id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
            dataUrl: scaled,
            mediaType: scaled.startsWith("data:image/jpeg")
              ? "image/jpeg"
              : f.mediaType ?? "image/jpeg",
            filename: f.filename,
          });
        }
        sendMessage(message.text, attachments.length > 0 ? attachments : undefined);
      })();
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
        "relative flex h-full w-full flex-col",
        "bg-surface",
      )}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {isDraggingImage && (
        <div className="pointer-events-none absolute inset-2 z-20 flex items-center justify-center rounded-lg border-2 border-dashed border-brand/60 bg-brand/10">
          <div className="text-[11px] font-semibold text-brand">
            Drop image to attach
          </div>
        </div>
      )}
      {/* Header with tabs */}
      <div className="flex items-center gap-1 px-2 py-1.5 border-b border-border/40 shrink-0 select-none">
        <button
          onClick={() => setActiveTab("chat")}
          className={cn(
            "px-2 py-1 text-[10px] font-semibold rounded transition-colors",
            activeTab === "chat"
              ? "bg-brand/10 text-brand"
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
              ? "bg-brand/10 text-brand"
              : "text-text-muted hover:text-text hover:bg-hover"
          )}
        >
          Source
        </button>
        <div className="flex-1" />
        {activeTab === "chat" && (
          <Tooltip content="New thread" side="bottom">
            <button
              onClick={clearThread}
              className="flex items-center gap-1 px-1.5 py-0.5 text-[10px] text-text-muted hover:text-text hover:bg-hover rounded transition-colors"
            >
              <Plus size={11} />
              New
            </button>
          </Tooltip>
        )}
        <Tooltip content="Close" side="bottom">
          <button
            onClick={() => setOpen(false)}
            className="flex h-5 w-5 items-center justify-center text-text-muted hover:text-text hover:bg-hover rounded transition-colors"
          >
            <X size={13} />
          </button>
        </Tooltip>
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
                <VcadMessage key={msg.id} msg={msg} userName={userName} />
              ))}
              {showShimmer && (
                <Shimmer className="px-1 text-[11px]">Thinking...</Shimmer>
              )}
            </ConversationContent>
            <ConversationScrollButton />
          </Conversation>

          {/* Anon-only sign-in banner. The `!isAuthenticated` guard is what
              keeps a signed-in user from seeing "Free chat limit reached"
              if a stale-token request gets routed to the anon rate limit
              before the auto-refresh kicks in. */}
          {usageError?.kind === "anon_limit" && !isAuthenticated && (
            <div className="shrink-0 bg-brand/10 px-4 py-2 text-[10px] text-text">
              <div className="mb-0.5 font-semibold text-brand">Free trial limit reached</div>
              <div className="text-text-muted">{usageError.message}</div>
              <button
                onClick={() => setShowAuthModal(true)}
                className="mt-1 rounded bg-brand px-2 py-0.5 text-[9px] text-white hover:bg-brand/90"
              >
                Sign in to continue
              </button>
            </div>
          )}
          {!isAuthenticated && anonUsage.used > 0 && !usageError && (
            <AnonTrialBar used={anonUsage.used} limit={anonUsage.limit} />
          )}

          {/* Compose dock — suggestions + input grouped in one padded surface,
              no dividing borders. The input's own border is the only line. */}
          <div className="shrink-0 space-y-2.5 px-3 pb-3 pt-1">
            <CadSuggestions
              selection={selectionContext}
              onPick={handleSuggestionPick}
            />

            <div ref={inputWrapRef}>
              <PromptInput onSubmit={handlePromptSubmit} accept="image/*">
                <ChatAttachmentBridge />
                {selectionContext.length > 0 && (
                  <PromptInputHeader>
                    {selectionContext.map((ctx) => (
                      <span
                        key={ctx.partId}
                        className="inline-flex items-center gap-1 rounded-full border border-border bg-fill py-0.5 pl-2 pr-1 text-[10px] font-mono text-text-muted"
                      >
                        {ctx.partName}
                        <button
                          onClick={() => removeContextPart(ctx.partId)}
                          className="flex h-3 w-3 items-center justify-center rounded-full hover:bg-hover hover:text-text transition-colors"
                          aria-label={`Remove ${ctx.partName}`}
                        >
                          <X size={8} />
                        </button>
                      </span>
                    ))}
                  </PromptInputHeader>
                )}
                <AttachmentPreviewStrip />
                <PromptInputTextarea
                  placeholder={placeholder}
                  disabled={streaming}
                  className="text-[11px]"
                />
                <PromptInputFooter>
                  <PromptInputTools>
                    <AttachViewportButton />
                  </PromptInputTools>
                  <PromptInputSubmit
                    status={submitStatus}
                    onStop={() => useChatStore.getState().requestCancel()}
                  />
                </PromptInputFooter>
              </PromptInput>
            </div>
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

      {/* Upgrade modal — opens on 429 monthly_limit or when the meter's
          Upgrade button is clicked. Dismissing clears the limit error so the
          user can retry after upgrading. */}
      <UpgradeModal
        open={showUpgradeModal}
        onOpenChange={(v) => {
          setShowUpgradeModal(v);
          if (!v && usageError?.kind === "monthly_limit") {
            setUsageError(null);
          }
        }}
        reason={usageError?.kind === "monthly_limit" ? "limit-reached" : "manual"}
      />
    </div>
  );
}
