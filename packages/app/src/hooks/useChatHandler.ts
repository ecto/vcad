import { useEffect, useCallback } from "react";
import { useChatStore, useDocumentStore, useUiStore, commandRegistry, executeCrud } from "@vcad/core";
import type { SelectionContext, ToolCallInfo, MessagePart, ExecutionResult } from "@vcad/core";
import { streamChat } from "@/lib/chat-api";
import type { ToolCall, ChatRequestMessage } from "@/lib/chat-api";

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
    }, { tools, systemPrompt });
  });
}

export function useChatHandler() {
  const handleChatSend = useCallback(
    async (e: CustomEvent<{ content: string; context: SelectionContext[] }>) => {
      const { content, context } = e.detail;
      const store = useChatStore.getState();

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

      const accumulatedToolCalls: ToolCallInfo[] = [];
      const parts: MessagePart[] = [];
      let fullText = "";
      const MAX_TOOL_LOOPS = 25;

      const updateUI = () => {
        useChatStore.getState().updateLastAssistant(fullText, accumulatedToolCalls, [...parts]);
      };

      try {
        for (let loop = 0; loop < MAX_TOOL_LOOPS; loop++) {
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
          });

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
      }
    },
    [],
  );

  useEffect(() => {
    const handler = (e: Event) =>
      handleChatSend(e as CustomEvent<{ content: string; context: SelectionContext[] }>);
    window.addEventListener("vcad:chat-send", handler);
    return () => window.removeEventListener("vcad:chat-send", handler);
  }, [handleChatSend]);
}
