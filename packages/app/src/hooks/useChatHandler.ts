import { useEffect, useCallback } from "react";
import { useChatStore } from "@vcad/core";
import type { SelectionContext } from "@vcad/core";
import { streamChat } from "@/lib/chat-api";

export function useChatHandler() {
  const handleChatSend = useCallback(
    async (e: CustomEvent<{ content: string; context: SelectionContext[] }>) => {
      const { content, context } = e.detail;
      const store = useChatStore.getState();

      // Build history from prior messages (exclude empty), then append the new user message
      const prior = store.messages
        .filter((msg) => msg.content.trim() !== "")
        .slice(-19)
        .map((msg) => ({
          role: msg.role as "user" | "assistant",
          content: msg.content,
        }));
      const history = [...prior, { role: "user" as const, content }];

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
    const handler = (e: Event) => handleChatSend(e as CustomEvent<{ content: string; context: SelectionContext[] }>);
    window.addEventListener("vcad:chat-send", handler);
    return () => window.removeEventListener("vcad:chat-send", handler);
  }, [handleChatSend]);
}
