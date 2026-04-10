import { useEffect, useCallback } from "react";
import { useChatStore, useDocumentStore, useUiStore } from "@vcad/core";
import type { SelectionContext, ToolCallInfo, MessagePart } from "@vcad/core";
import { streamChat } from "@/lib/chat-api";
import type { ToolCall, ChatRequestMessage } from "@/lib/chat-api";

/**
 * Execute a tool call against the document/UI stores.
 * Returns a string result for display in the chat.
 */
function executeTool(tool: ToolCall): { result: string; status: "success" | "error" } {
  const docStore = useDocumentStore.getState();
  const uiStore = useUiStore.getState();

  try {
    switch (tool.name) {
      case "add_primitive": {
        const kind = tool.args.kind as "cube" | "cylinder" | "sphere";
        const partId = docStore.addPrimitive(kind);
        uiStore.select(partId);
        uiStore.setTransformMode("translate");
        return { result: `Added ${kind} with id: ${partId}`, status: "success" };
      }

      case "transform_part": {
        const partId = tool.args.partId as string;
        if (tool.args.translate) {
          const t = tool.args.translate as { x: number; y: number; z: number };
          docStore.setTranslation(partId, { x: t.x ?? 0, y: t.y ?? 0, z: t.z ?? 0 });
        }
        if (tool.args.rotate) {
          const r = tool.args.rotate as { x: number; y: number; z: number };
          docStore.setRotation(partId, { x: r.x ?? 0, y: r.y ?? 0, z: r.z ?? 0 });
        }
        if (tool.args.scale) {
          const s = tool.args.scale as { x: number; y: number; z: number };
          docStore.setScale(partId, { x: s.x ?? 1, y: s.y ?? 1, z: s.z ?? 1 });
        }
        return { result: `Transformed ${partId}`, status: "success" };
      }

      case "add_fillet": {
        const id = docStore.addFillet(tool.args.partId as string, tool.args.radius as number);
        return id
          ? { result: `Applied ${tool.args.radius}mm fillet`, status: "success" }
          : { result: "Fillet failed — select a valid solid part", status: "error" };
      }

      case "add_chamfer": {
        const id = docStore.addChamfer(tool.args.partId as string, tool.args.distance as number);
        return id
          ? { result: `Applied ${tool.args.distance}mm chamfer`, status: "success" }
          : { result: "Chamfer failed — select a valid solid part", status: "error" };
      }

      case "add_shell": {
        const id = docStore.addShell(tool.args.partId as string, tool.args.thickness as number);
        return id
          ? { result: `Shelled with ${tool.args.thickness}mm walls`, status: "success" }
          : { result: "Shell failed — select a valid solid part", status: "error" };
      }

      case "apply_boolean": {
        const selectedIds = Array.from(uiStore.selectedPartIds);
        if (selectedIds.length !== 2) {
          return { result: "Boolean requires exactly 2 parts selected", status: "error" };
        }
        const op = tool.args.operation as "union" | "difference" | "intersection";
        docStore.applyBoolean(op, selectedIds[0]!, selectedIds[1]!);
        return { result: `Applied ${op}`, status: "success" };
      }

      case "delete_part": {
        docStore.removePart(tool.args.partId as string);
        uiStore.clearSelection();
        return { result: `Deleted part`, status: "success" };
      }

      case "inspect_part": {
        const partId = tool.args.partId as string;
        const part = docStore.partIndex.get(partId);
        if (!part) return { result: `Part ${partId} not found`, status: "error" };
        return {
          result: JSON.stringify({
            id: part.id,
            name: part.name,
            kind: part.kind,
          }),
          status: "success",
        };
      }

      case "list_parts": {
        const parts = docStore.parts.map((p) => ({
          id: p.id,
          name: p.name,
          kind: p.kind,
        }));
        return { result: JSON.stringify(parts), status: "success" };
      }

      default:
        return { result: `Unknown tool: ${tool.name}`, status: "error" };
    }
  } catch (err) {
    return {
      result: err instanceof Error ? err.message : "Tool execution failed",
      status: "error",
    };
  }
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

    streamChat(history, context, {
      onText: (t) => {
        text = t;
        onStreamText(t);
      },
      onToolCall: (tool) => {
        toolCalls.push(tool);
      },
      onError: (err) => {
        error = err;
      },
      onFinish: () => {
        resolve({ text, toolCalls, error });
      },
    });
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
      const MAX_TOOL_LOOPS = 5;

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
            const { result, status } = executeTool(tool);
            toolResults.push({ id: tool.id, result, status });
            const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
            if (entry) {
              entry.result = result;
              entry.status = status;
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
