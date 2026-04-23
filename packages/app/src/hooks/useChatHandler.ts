import { useEffect, useCallback, useRef } from "react";
import {
  useChatStore,
  useDocumentStore,
  useUiStore,
  useParticipantStore,
  ensureAiParticipant,
  AI_PARTICIPANT_ID,
  commandRegistry,
  executeCrud,
  HIGH_LEVEL_TOOLS_SYSTEM_PROMPT_APPENDIX,
} from "@vcad/core";
import type { SelectionContext, ToolCallInfo, MessagePart, ExecutionResult, ChatUsageError, ChatAttachment, ChatMessage } from "@vcad/core";
import { persistToolResult, useAuthStore } from "@vcad/auth";
import { streamChat, LIMIT_ERROR_PREFIX } from "@/lib/chat-api";
import type { ToolCall, ChatRequestMessage } from "@/lib/chat-api";
import {
  SCREENSHOT_VIEWPORT_TOOL,
  SCREENSHOT_SYSTEM_PROMPT_APPENDIX,
  executeScreenshotViewport,
} from "@/lib/ai-screenshot";
import {
  AI_CAMERA_TOOL_NAMES,
  AI_CAMERA_SYSTEM_PROMPT_APPENDIX,
  executeAiCamera,
} from "@/lib/ai-camera-tools";
import {
  AI_DOCUMENT_TOOL_NAMES,
  AI_DOCUMENT_SYSTEM_PROMPT_APPENDIX,
  GET_DOCUMENT_NAME_TOOL,
  SET_DOCUMENT_NAME_TOOL,
  executeAiDocumentTool,
} from "@/lib/ai-document-tools";

/**
 * Parse a rate-limit error body emitted by streamChat with LIMIT_ERROR_PREFIX.
 * Returns null if the string isn't a rate-limit error or if the embedded
 * `error` field isn't a known kind. Unknown kinds previously fell through
 * to "anon_limit" and were rendered as "Free chat limit reached", which
 * masked auth/server errors as a sign-in problem.
 */
function parseLimitError(err: string): ChatUsageError | null {
  if (!err.startsWith(LIMIT_ERROR_PREFIX)) return null;
  const json = err.slice(LIMIT_ERROR_PREFIX.length);
  let parsed: {
    error?: string;
    message?: string;
    usage?: number;
    limit?: number;
    resets_at?: string;
  };
  try {
    parsed = JSON.parse(json);
  } catch {
    return null;
  }
  if (parsed.error !== "monthly_limit" && parsed.error !== "anon_limit") {
    return null;
  }
  const kind = parsed.error;
  return {
    kind,
    message:
      parsed.message ??
      (kind === "monthly_limit" ? "Monthly limit reached." : "Free limit reached."),
    usage: parsed.usage,
    limit: parsed.limit,
    resetsAt: parsed.resets_at,
  };
}

/**
 * Wrap `useUiStore` so that `select` / `selectMultiple` / `clearSelection`
 * calls made from a chat tool executor write to the AI participant's
 * selection instead of the human user's. This is how we model the AI as
 * a separate participant: the AI "selecting" a part it just created
 * becomes its own focus/highlight, and never disturbs the user's
 * viewport, property panel, or feature-tree selection.
 */
function uiStoreForAi(): ReturnType<typeof useUiStore.getState> {
  const real = useUiStore.getState();
  ensureAiParticipant();
  const participants = useParticipantStore.getState();
  const aiParticipant = participants.participants.get(AI_PARTICIPANT_ID);
  const aiSelection = aiParticipant?.selectedPartIds ?? new Set<string>();
  return {
    ...real,
    // Route selection *reads* to the AI participant too, so tools that fall
    // back to "whatever is selected" (e.g. booleans with no ids passed)
    // see the AI's focus, not the human user's.
    selectedPartIds: aiSelection,
    select: (partId) => {
      if (partId) participants.setSelection(AI_PARTICIPANT_ID, [partId]);
      else participants.clearSelection(AI_PARTICIPANT_ID);
    },
    selectMultiple: (partIds) =>
      participants.setSelection(AI_PARTICIPANT_ID, partIds),
    clearSelection: () => participants.clearSelection(AI_PARTICIPANT_ID),
  };
}

/**
 * Execute a tool call against the document/UI stores via the CRUD registry.
 * Returns the full ExecutionResult so display payload and duration can be propagated.
 */
function executeTool(tool: ToolCall): ExecutionResult {
  const docStore = useDocumentStore.getState();
  const uiStore = uiStoreForAi();
  return executeCrud(tool.name, tool.args, docStore, uiStore);
}

/** Split a `data:<media-type>;base64,<data>` URL into its parts. Returns
 * null if the URL isn't a base64 data URL (e.g. still a blob: URL). */
function splitDataUrl(
  dataUrl: string,
): { mediaType: string; base64: string } | null {
  const match = /^data:([^;]+);base64,(.*)$/.exec(dataUrl);
  if (!match) return null;
  return { mediaType: match[1]!, base64: match[2]! };
}

/** Build the `content` field for an Anthropic user message that may carry
 * attachments. Returns a plain string when there are no attachments so we
 * don't bloat the API payload; returns a multimodal content array otherwise.
 * Attachments that fail to decode are silently dropped — partial delivery is
 * better than failing the whole turn. */
function buildUserMessageContent(msg: ChatMessage): string | object[] {
  const attachments = msg.attachments ?? [];
  if (attachments.length === 0) return msg.content;

  const blocks: object[] = [];
  for (const a of attachments) {
    const parts = splitDataUrl(a.dataUrl);
    if (!parts) continue;
    blocks.push({
      type: "image",
      source: {
        type: "base64",
        media_type: a.mediaType || parts.mediaType,
        data: parts.base64,
      },
    });
  }
  if (msg.content.trim()) {
    blocks.push({ type: "text", text: msg.content });
  }
  // If every attachment failed to decode and there's no text, fall back to
  // the raw content so we don't emit an empty-block message (Anthropic
  // rejects those).
  if (blocks.length === 0) return msg.content;
  return blocks;
}

/**
 * Build a list of document parts for use in the system prompt.
 */
function getDocumentParts(): Array<{ id: string; name: string; kind: string }> {
  const docStore = useDocumentStore.getState();
  return docStore.parts.map((p) => ({
    id: p.id,
    name: p.name,
    kind: p.kind,
  }));
}

/**
 * Run a streaming chat turn. Returns the text content and any tool calls.
 * Tool calls are returned but NOT executed — caller decides when.
 */
function runTurn(
  history: ChatRequestMessage[],
  context: SelectionContext[],
  onStreamText: (text: string) => void,
  onMeta: (meta: { threadId: string; assistantMessageId: string }) => void,
  signal: AbortSignal,
  persistence: {
    threadId: string | null;
    documentId: string | null;
    userMessageId: string | null;
    parentMessageId: string | null;
    assistantMessageId: string | null;
  },
): Promise<{ text: string; toolCalls: ToolCall[]; error: string | null }> {
  return new Promise((resolve) => {
    let text = "";
    const toolCalls: ToolCall[] = [];
    let error: string | null = null;

    const tools = [
      ...commandRegistry.toAnthropicTools(),
      SCREENSHOT_VIEWPORT_TOOL,
      GET_DOCUMENT_NAME_TOOL,
      SET_DOCUMENT_NAME_TOOL,
    ];
    const systemPrompt =
      commandRegistry.buildSystemPrompt(getDocumentParts(), context) +
      HIGH_LEVEL_TOOLS_SYSTEM_PROMPT_APPENDIX +
      SCREENSHOT_SYSTEM_PROMPT_APPENDIX +
      AI_CAMERA_SYSTEM_PROMPT_APPENDIX +
      AI_DOCUMENT_SYSTEM_PROMPT_APPENDIX;

    streamChat(history, context, {
      onText: (t) => { text = t; onStreamText(t); },
      onToolCall: (tool) => { toolCalls.push(tool); },
      onError: (err) => { error = err; },
      onFinish: () => { resolve({ text, toolCalls, error }); },
      onMeta,
    }, {
      tools,
      systemPrompt,
      signal,
      threadId: persistence.threadId,
      documentId: persistence.documentId,
      userMessageId: persistence.userMessageId,
      parentMessageId: persistence.parentMessageId,
      assistantMessageId: persistence.assistantMessageId,
    });
  });
}

function makeMessageId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

export function useChatHandler() {
  const handleChatSend = useCallback(
    async (
      content: string,
      context: SelectionContext[],
      attachments?: ChatAttachment[],
    ) => {
      const store = useChatStore.getState();

      const session = useAuthStore.getState().session;
      const isAnonymous = useAuthStore.getState().isAnonymous;
      // "Anon" for rate-limit purposes means no permanent identity. An
      // anonymous Supabase session still counts as anon here so the IP-based
      // 3-message-per-day cap applies, but it has a real auth.uid() so
      // chat threads persist under that uid.
      const isAnon = !session || isAnonymous;

      // Persistence context — pulled from chat-store (set by useChatHydration)
      // and document-store. When threadId is null (Supabase not configured,
      // no document open), the server falls back to the legacy in-memory
      // path and writes nothing.
      const threadId = useChatStore.getState().threadId;
      const documentId = useDocumentStore.getState().documentId;
      // Track parent across turns; updated from the meta event each turn so
      // the next user message points at the right ancestor.
      let parentMessageId: string | null = null;
      const lastAssistant = [...useChatStore.getState().messages]
        .reverse()
        .find((m) => m.role === "assistant" && m.id !== "welcome");
      if (lastAssistant) parentMessageId = lastAssistant.id;

      // Defense-in-depth: hard-block anon sends once the local counter hits
      // the limit. This prevents runaway cost if the server-side rate limit
      // is misconfigured (e.g. missing SUPABASE_SERVICE_ROLE_KEY in prod).
      // The server is still the source of truth for the limit, but this
      // keeps the client honest even when auth isn't configured at all.
      if (isAnon && store.anonUsage.used >= store.anonUsage.limit) {
        store.setUsageError({
          kind: "anon_limit",
          message: `You've used your ${store.anonUsage.limit} free chat messages. Sign in for more.`,
          limit: store.anonUsage.limit,
          usage: store.anonUsage.used,
        });
        return;
      }

      // If a previous request already reported a limit error, don't retry —
      // the server will just 429 again.
      if (isAnon && store.usageError?.kind === "anon_limit") {
        // The sidebar will handle opening the auth modal for this case.
        return;
      }
      if (!isAnon && store.usageError?.kind === "monthly_limit") {
        // Banner is already shown; nothing to do.
        return;
      }

      // Pre-generate ids so the local optimistic rows share ids with the
      // server-persisted rows. Without this, Realtime upserts would create
      // duplicate bubbles next to the local placeholders.
      const firstUserMessageId = makeMessageId();
      const firstAssistantMessageId = makeMessageId();

      // Add the user message to the store with the same id we'll send to
      // the server as user_message_id.
      store.addUserMessage(content, context, attachments, firstUserMessageId);

      // Build base history from store. Text messages flow through as plain
      // strings; user messages with attached images become multimodal content
      // arrays so Claude actually sees the screenshots.
      const allMessages = useChatStore.getState().messages;
      const baseHistory: ChatRequestMessage[] = [];
      for (const msg of allMessages) {
        const hasAttachments = (msg.attachments?.length ?? 0) > 0;
        if (msg.content.trim() === "" && !hasAttachments) continue;
        const messageContent =
          msg.role === "user"
            ? buildUserMessageContent(msg)
            : msg.content;
        baseHistory.push({
          role: msg.role as "user" | "assistant",
          content: messageContent,
        });
      }
      const history: ChatRequestMessage[] = baseHistory.slice(-20);

      // Add empty placeholder for streaming response. Uses the same id we'll
      // send the server as assistant_message_id so Realtime updates merge
      // with this placeholder instead of creating a sibling row.
      store.addAssistantMessage("", undefined, firstAssistantMessageId);
      // Mark this id as locally-streamed so useChatHydration's reproject
      // doesn't overwrite our SSE-driven render with the (slower, possibly
      // empty) DB version.
      store.markLocallyStreaming(firstAssistantMessageId);
      store.setStreaming(true);
      store.setError(null);
      store.setUsageError(null);
      store.clearCancel();

      // The AI participant joins the document for this turn. If this is
      // the first time we've seen them in this session, default to Follow
      // so the user sees the AI camera as a frustum in-scene (Lock is
      // opt-in via the toggle). After the user has made an explicit
      // choice, don't override it on subsequent turns.
      const firstAppearance = !useParticipantStore
        .getState()
        .participants.has(AI_PARTICIPANT_ID);
      ensureAiParticipant();
      if (firstAppearance) {
        const uiState = useUiStore.getState();
        uiState.setFollowMode("follow");
        uiState.setFollowingParticipant(AI_PARTICIPANT_ID);
      }

      // Count the send against the local anon badge as soon as it leaves the
      // client. The server is the source of truth; this is just the UX hint.
      if (isAnon) {
        store.incAnonUsage();
      }

      const accumulatedToolCalls: ToolCallInfo[] = [];
      const parts: MessagePart[] = [];
      let fullText = "";
      const abortController = new AbortController();
      // Expose the abort controller so requestCancel() can interrupt the
      // in-flight fetch immediately instead of waiting for the current turn
      // to finish naturally.
      useChatStore.getState().setAbortController(abortController);

      const updateUI = () => {
        useChatStore.getState().updateLastAssistant(fullText, accumulatedToolCalls, [...parts]);
      };

      // Generated fresh per turn. The first turn uses the ids we already
      // generated (so they match the local placeholder rows); subsequent
      // turns are tool-result continuations and the server skips writing
      // those as user messages (results live on chat_tool_calls rows).
      let nextUserMessageId = firstUserMessageId;
      let nextAssistantMessageId: string = firstAssistantMessageId;

      try {
        while (true) {
          // Check for user cancellation before each turn
          if (useChatStore.getState().cancelRequested) {
            abortController.abort();
            parts.push({ type: "text", text: "_[Stopped by user]_" });
            updateUI();
            break;
          }
          // Stream a turn — text gets appended to the current text part
          const { text, toolCalls, error } = await runTurn(
            history,
            context,
            (streamedText) => {
              // Replace the trailing text part with the latest streamed text
              const lastPart = parts[parts.length - 1];
              if (lastPart?.type === "text") {
                lastPart.text = streamedText;
              } else if (streamedText) {
                parts.push({ type: "text", text: streamedText });
              }
              fullText = parts.filter((p) => p.type === "text").map((p) => (p as { type: "text"; text: string }).text).join("\n\n");
              updateUI();
            },
            (meta) => {
              // Server assigned an assistant message id. Use it as the
              // parent for the next user message (or tool-result turn) and
              // remember it so persisted tool results are correctly tied
              // back to this turn.
              parentMessageId = meta.assistantMessageId;
            },
            abortController.signal,
            {
              threadId,
              documentId,
              userMessageId: nextUserMessageId,
              parentMessageId,
              assistantMessageId: nextAssistantMessageId,
            },
          );

          // Finalize this turn's text part
          if (text.trim()) {
            const lastPart = parts[parts.length - 1];
            if (lastPart?.type === "text") {
              lastPart.text = text;
            } else {
              parts.push({ type: "text", text });
            }
          }
          fullText = parts.filter((p) => p.type === "text").map((p) => (p as { type: "text"; text: string }).text).join("\n\n");

          // Mid-stream cancel: the abort took effect during runTurn. Mark the
          // message as stopped and exit the loop. (The top-of-loop check
          // handles cancellation that lands cleanly between turns.)
          if (useChatStore.getState().cancelRequested) {
            parts.push({ type: "text", text: "_[Stopped by user]_" });
            updateUI();
            break;
          }

          if (error) {
            // Detect rate-limit errors from the server and route them to the
            // usageError state (which the sidebar turns into a banner / modal
            // trigger) instead of showing a generic error message.
            const limit = parseLimitError(error);
            // Defense-in-depth: an `anon_limit` reply for a permanently
            // signed-in user means the request was treated as anonymous on
            // the server (almost always a stale token or transient auth
            // blip). The misleading "Free chat limit reached / Sign in"
            // banner is the wrong UX for an authed user — surface it as a
            // generic error so they retry instead.
            if (limit && limit.kind === "anon_limit" && !isAnon) {
              const msg = "Authentication issue — please retry in a moment.";
              parts.push({ type: "text", text: `Error: ${msg}` });
              fullText += `\n\nError: ${msg}`;
              useChatStore.getState().setError(msg);
              updateUI();
              break;
            }
            if (limit) {
              useChatStore.getState().setUsageError(limit);
              // Don't pollute the chat with an Error: line — the banner shows.
              updateUI();
              break;
            }
            parts.push({ type: "text", text: `Error: ${error}` });
            fullText += `\n\nError: ${error}`;
            useChatStore.getState().setError(error);
            updateUI();
            break;
          }

          // If no tool calls, we're done
          if (toolCalls.length === 0) {
            updateUI();
            break;
          }

          // Add tool calls as pending parts (chronologically after the text)
          for (const tool of toolCalls) {
            const info: ToolCallInfo = {
              id: tool.id,
              name: tool.name,
              args: tool.args,
              result: undefined,
              status: "pending",
            };
            accumulatedToolCalls.push(info);
            parts.push({ type: "tool", tool: info });
          }
          updateUI();

          // Defer tool execution to next tick
          await new Promise<void>((resolve) => setTimeout(resolve, 0));

          // Execute tools and update their status in-place. Tool results sent
          // back to the model may be a plain string (CRUD tools) or an array
          // of Anthropic content blocks (screenshot tool — image + text).
          const toolResults: Array<{
            id: string;
            content: string | object[];
            status: "success" | "error";
          }> = [];
          for (const tool of toolCalls) {
            if (tool.name === "screenshot_viewport") {
              const shot = await executeScreenshotViewport(tool.args);
              toolResults.push({
                id: tool.id,
                content: shot.toolResultContent ?? shot.result,
                status: shot.status,
              });
              const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
              if (entry) {
                entry.result = shot.result;
                entry.status = shot.status;
                entry.duration = shot.duration;
                if (shot.imageDataUrl) entry.imageDataUrl = shot.imageDataUrl;
              }
            } else if (AI_CAMERA_TOOL_NAMES.has(tool.name)) {
              const exec = executeAiCamera(tool);
              toolResults.push({ id: tool.id, content: exec.result, status: exec.status });
              const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
              if (entry) {
                entry.result = exec.result;
                entry.status = exec.status;
                entry.display = exec.display;
                entry.duration = exec.duration;
              }
            } else if (AI_DOCUMENT_TOOL_NAMES.has(tool.name)) {
              const exec = executeAiDocumentTool(tool);
              toolResults.push({ id: tool.id, content: exec.result, status: exec.status });
              const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
              if (entry) {
                entry.result = exec.result;
                entry.status = exec.status;
                entry.display = exec.display;
                entry.duration = exec.duration;
              }
            } else {
              const exec = executeTool(tool);
              toolResults.push({ id: tool.id, content: exec.result, status: exec.status });
              const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
              if (entry) {
                entry.result = exec.result;
                entry.status = exec.status;
                entry.display = exec.display;
                entry.duration = exec.duration;
              }
            }
          }
          updateUI();

          // Persist the tool execution results to the server so subsequent
          // turns (and other tabs) see them. Fire-and-forget — failure to
          // persist doesn't block the in-memory loop. Skip when there's no
          // thread (Supabase unconfigured) — the server wouldn't have
          // written the tool_call row anyway.
          if (threadId) {
            for (const entry of accumulatedToolCalls) {
              if (entry.status === "pending") continue;
              if (entry.result === undefined) continue;
              void persistToolResult(entry.id, {
                result: entry.result,
                status: entry.status,
                display: entry.display,
                imageDataUrl: entry.imageDataUrl,
                durationMs: entry.duration,
              });
            }
          }

          // Roll the ids for the next (tool-result-continuation) turn.
          // Server will detect tool-result-only content and skip writing
          // the user msg as a UI row, but the id is still threaded
          // through for logging consistency. The assistant id, however,
          // becomes the row id of the next streaming response — the local
          // placeholder for it is created below before the loop continues.
          parentMessageId = nextAssistantMessageId;
          // Stop protecting the just-completed turn from Realtime updates;
          // the DB row is now the canonical source.
          useChatStore.getState().unmarkLocallyStreaming(nextAssistantMessageId);
          nextUserMessageId = makeMessageId();
          nextAssistantMessageId = makeMessageId();
          store.addAssistantMessage("", undefined, nextAssistantMessageId);
          useChatStore.getState().markLocallyStreaming(nextAssistantMessageId);
          // Reset accumulators for the next turn's parts. Without this the
          // continuation overwrites the previous turn's tool chips.
          parts.length = 0;
          accumulatedToolCalls.length = 0;
          fullText = "";

          // Append the assistant turn (with text + tool uses) and the user turn (with tool results)
          // to the history for the follow-up request.
          const assistantContent: Array<{ type: string; [k: string]: unknown }> = [];
          if (text.trim()) {
            assistantContent.push({ type: "text", text });
          }
          for (const tool of toolCalls) {
            assistantContent.push({
              type: "tool_use",
              id: tool.id,
              name: tool.name,
              input: tool.args,
            });
          }
          history.push({ role: "assistant", content: assistantContent });

          const userContent = toolResults.map((r) => ({
            type: "tool_result",
            tool_use_id: r.id,
            content: r.content,
            is_error: r.status === "error",
          }));
          history.push({ role: "user", content: userContent });

          // Loop: stream the follow-up turn
        }
      } finally {
        useChatStore.getState().setStreaming(false);
        useChatStore.getState().clearCancel();
        useChatStore.getState().setAbortController(null);
        // Release the protection on the trailing assistant turn so that
        // future Realtime updates (e.g. server marking it interrupted via
        // a sweep) actually take effect.
        useChatStore.getState().unmarkLocallyStreaming(nextAssistantMessageId);
      }
    },
    [],
  );

  // Register the handler with the chat store so any UI component can call
  // useChatStore.getState().sendMessage() without depending on this hook
  // directly. Hook still has to be mounted at App root so the registration
  // survives across re-renders of UI components that fire sends.
  useEffect(() => {
    useChatStore.getState().setSendHandler(handleChatSend);
    return () => useChatStore.getState().setSendHandler(null);
  }, [handleChatSend]);

  // Retire the AI participant (camera frustum + selection highlight in the
  // viewport) shortly after a chat turn ends. Debounced because `streaming`
  // toggles within a single turn (LLM pass → tool call → next LLM pass);
  // we don't want the frustum flickering between passes. The DocTitle
  // presence pill is gated on `streaming` directly and doesn't flicker —
  // this cleanup is only for the lingering viewport state.
  const cleanupTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    const unsubscribe = useChatStore.subscribe((state, prev) => {
      if (state.streaming === prev.streaming) return;
      if (state.streaming) {
        // New pass started — cancel any pending cleanup.
        if (cleanupTimerRef.current) {
          clearTimeout(cleanupTimerRef.current);
          cleanupTimerRef.current = null;
        }
      } else {
        // Streaming paused. If the whole turn is done, tear down AI presence.
        cleanupTimerRef.current = setTimeout(() => {
          useParticipantStore.getState().remove(AI_PARTICIPANT_ID);
          cleanupTimerRef.current = null;
        }, 800);
      }
    });
    return () => {
      unsubscribe();
      if (cleanupTimerRef.current) {
        clearTimeout(cleanupTimerRef.current);
        cleanupTimerRef.current = null;
      }
    };
  }, []);
}
