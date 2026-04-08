import type { SelectionContext } from "@vcad/core";

export interface ChatRequestMessage {
  role: "user" | "assistant";
  content: string;
}

export interface ChatStreamCallbacks {
  onText: (text: string) => void;
  onError: (error: string) => void;
  onFinish: () => void;
}

export async function streamChat(
  messages: ChatRequestMessage[],
  context: SelectionContext[],
  callbacks: ChatStreamCallbacks,
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

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      const chunk = decoder.decode(value, { stream: true });
      fullText += chunk;
      callbacks.onText(fullText);
    }

    callbacks.onFinish();
  } catch (err) {
    callbacks.onError(err instanceof Error ? err.message : "Stream failed");
    callbacks.onFinish();
  }
}
