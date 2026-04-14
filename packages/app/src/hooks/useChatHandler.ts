import { useEffect, useCallback } from "react";
import { useChatStore, useDocumentStore, useUiStore, commandRegistry, executeCrud } from "@vcad/core";
import type { SelectionContext, ToolCallInfo, MessagePart, ExecutionResult, ChatUsageError } from "@vcad/core";
import { useAuthStore } from "@vcad/auth";
import { streamChat, LIMIT_ERROR_PREFIX } from "@/lib/chat-api";
import type { ToolCall, ChatRequestMessage } from "@/lib/chat-api";

/**
 * Parse a rate-limit error body emitted by streamChat with LIMIT_ERROR_PREFIX.
 * Returns null if the string isn't a rate-limit error.
 */
function parseLimitError(err: string): ChatUsageError | null {
  if (!err.startsWith(LIMIT_ERROR_PREFIX)) return null;
  const json = err.slice(LIMIT_ERROR_PREFIX.length);
  try {
    const parsed = JSON.parse(json) as {
      error?: string;
      message?: string;
      usage?: number;
      limit?: number;
      resets_at?: string;
    };
    const kind = parsed.error === "monthly_limit" ? "monthly_limit" : "anon_limit";
    return {
      kind,
      message: parsed.message ?? (kind === "monthly_limit" ? "Monthly limit reached." : "Free limit reached."),
      usage: parsed.usage,
      limit: parsed.limit,
      resetsAt: parsed.resets_at,
    };
  } catch {
    return null;
  }
}

/**
 * Execute a tool call against the document/UI stores via the CRUD registry.
 * Returns the full ExecutionResult so display payload and duration can be propagated.
 */
function executeTool(tool: ToolCall): ExecutionResult {
  const docStore = useDocumentStore.getState();
  const uiStore = useUiStore.getState();
  return executeCrud(tool.name, tool.args, docStore, uiStore);
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
  signal: AbortSignal,
): Promise<{ text: string; toolCalls: ToolCall[]; error: string | null }> {
  return new Promise((resolve) => {
    let text = "";
    const toolCalls: ToolCall[] = [];
    let error: string | null = null;

    const tools = commandRegistry.toAnthropicTools();
    const systemPrompt = commandRegistry.buildSystemPrompt(getDocumentParts(), context);

    streamChat(history, context, {
      onText: (t) => { text = t; onStreamText(t); },
      onToolCall: (tool) => { toolCalls.push(tool); },
      onError: (err) => { error = err; },
      onFinish: () => { resolve({ text, toolCalls, error }); },
    }, { tools, systemPrompt, signal });
  });
}

export function useChatHandler() {
  const handleChatSend = useCallback(
    async (content: string, context: SelectionContext[]) => {
      const store = useChatStore.getState();

      const session = useAuthStore.getState().session;
      const isAnon = !session;

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

      // Add the user message to the store
      store.addUserMessage(content, context);

      // Build base history from store (text-only messages)
      const allMessages = useChatStore.getState().messages;
      const baseHistory: ChatRequestMessage[] = [];
      for (const msg of allMessages) {
        if (msg.content.trim() === "") continue;
        baseHistory.push({ role: msg.role as "user" | "assistant", content: msg.content });
      }
      const history: ChatRequestMessage[] = baseHistory.slice(-20);

      // Add empty placeholder for streaming response
      store.addAssistantMessage("");
      store.setStreaming(true);
      store.setError(null);
      store.setUsageError(null);
      store.clearCancel();

      // Count the send against the local anon badge as soon as it leaves the
      // client. The server is the source of truth; this is just the UX hint.
      if (isAnon) {
        store.incAnonUsage();
      }

      const accumulatedToolCalls: ToolCallInfo[] = [];
      const parts: MessagePart[] = [];
      let fullText = "";
      const abortController = new AbortController();

      const updateUI = () => {
        useChatStore.getState().updateLastAssistant(fullText, accumulatedToolCalls, [...parts]);
      };

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
          const { text, toolCalls, error } = await runTurn(history, context, (streamedText) => {
            // Replace the trailing text part with the latest streamed text
            const lastPart = parts[parts.length - 1];
            if (lastPart?.type === "text") {
              lastPart.text = streamedText;
            } else if (streamedText) {
              parts.push({ type: "text", text: streamedText });
            }
            fullText = parts.filter((p) => p.type === "text").map((p) => (p as { type: "text"; text: string }).text).join("\n\n");
            updateUI();
          }, abortController.signal);

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

          if (error) {
            // Detect rate-limit errors from the server and route them to the
            // usageError state (which the sidebar turns into a banner / modal
            // trigger) instead of showing a generic error message.
            const limit = parseLimitError(error);
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

          // Execute tools and update their status in-place
          const toolResults: Array<{ id: string; result: string; status: "success" | "error" }> = [];
          for (const tool of toolCalls) {
            const exec = executeTool(tool);
            toolResults.push({ id: tool.id, result: exec.result, status: exec.status });
            const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
            if (entry) {
              entry.result = exec.result;
              entry.status = exec.status;
              entry.display = exec.display;
              entry.duration = exec.duration;
            }
          }
          updateUI();

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
            content: r.result,
            is_error: r.status === "error",
          }));
          history.push({ role: "user", content: userContent });

          // Loop: stream the follow-up turn
        }
      } finally {
        useChatStore.getState().setStreaming(false);
        useChatStore.getState().clearCancel();
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
}
