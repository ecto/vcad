/**
 * Local AI bridge.
 *
 * In desktop mode we can reach Ollama / llama.cpp / MLX running on
 * localhost. Routing the traffic through Tauri (rather than direct
 * `fetch`) means:
 *   - The webview CSP stays tight — no `http://127.0.0.1:*` hole.
 *   - We can later extend to server management / other engines without
 *     touching the chat UI.
 *
 * In browser mode these helpers throw — callers should check
 * `useCapabilities().localAi.ollama` before invoking.
 */

import { Channel } from "@tauri-apps/api/core";
import { isTauri, invoke } from "@/lib/tauri";

export interface LocalAiMessage {
  role: "user" | "assistant" | "system";
  content: string;
}

export type LocalAiEvent =
  | { kind: "delta"; text: string }
  | { kind: "done" }
  | { kind: "error"; message: string };

export interface LocalAiStreamCallbacks {
  onText: (delta: string, full: string) => void;
  onFinish: () => void;
  onError: (msg: string) => void;
}

/** Stream a chat completion from a locally-running Ollama. */
export async function streamLocalAiChat(
  model: string,
  messages: LocalAiMessage[],
  callbacks: LocalAiStreamCallbacks,
): Promise<void> {
  if (!isTauri()) {
    callbacks.onError("local AI is only available in the desktop build");
    callbacks.onFinish();
    return;
  }

  const onEvent = new Channel<LocalAiEvent>();
  let full = "";
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    callbacks.onFinish();
  };

  onEvent.onmessage = (event) => {
    switch (event.kind) {
      case "delta":
        full += event.text;
        callbacks.onText(event.text, full);
        break;
      case "done":
        finish();
        break;
      case "error":
        callbacks.onError(event.message);
        finish();
        break;
    }
  };

  try {
    await invoke<void>("local_ai_chat_stream", { model, messages, onEvent });
    finish();
  } catch (err) {
    callbacks.onError(err instanceof Error ? err.message : String(err));
    finish();
  }
}
