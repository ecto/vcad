import type { SelectionContext, AnthropicTool } from "@vcad/core";

export interface ChatRequestMessage {
  role: "user" | "assistant";
  content: string | object[];
}

export interface ToolCall {
  id: string;
  name: string;
  args: Record<string, unknown>;
}

export interface ChatStreamCallbacks {
  onText: (text: string) => void;
  onToolCall: (tool: ToolCall) => void;
  onError: (error: string) => void;
  onFinish: () => void;
}

export async function streamChat(
  messages: ChatRequestMessage[],
  context: SelectionContext[],
  callbacks: ChatStreamCallbacks,
  options?: {
    tools?: AnthropicTool[];
    systemPrompt?: string;
  },
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
        tools: options?.tools,
        systemPrompt: options?.systemPrompt,
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
    let fullText = "";
    let buffer = "";
    let currentToolId = "";
    let currentToolName = "";
    let currentToolJson = "";

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";

      for (const line of lines) {
        if (!line.startsWith("data: ")) continue;
        try {
          const event = JSON.parse(line.slice(6));
          switch (event.type) {
            case "text":
              fullText += event.text;
              callbacks.onText(fullText);
              break;
            case "tool_start":
              currentToolId = event.id;
              currentToolName = event.name;
              currentToolJson = "";
              break;
            case "tool_delta":
              currentToolJson += event.json;
              break;
            case "block_stop":
              if (currentToolId && currentToolName) {
                let args: Record<string, unknown> = {};
                try { args = JSON.parse(currentToolJson); } catch { /* empty args */ }
                callbacks.onToolCall({
                  id: currentToolId,
                  name: currentToolName,
                  args,
                });
                currentToolId = "";
                currentToolName = "";
                currentToolJson = "";
              }
              break;
            case "done":
              break;
          }
        } catch { /* skip parse errors */ }
      }
    }

    callbacks.onFinish();
  } catch (err) {
    callbacks.onError(err instanceof Error ? err.message : "Stream failed");
    callbacks.onFinish();
  }
}
