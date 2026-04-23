import type { SelectionContext, AnthropicTool } from "@vcad/core";
import { getSupabase, useAuthStore } from "@vcad/auth";

/**
 * Prefix used to signal a rate-limit error payload to the chat handler.
 * The handler can detect this and route to the auth modal / banner instead
 * of showing a generic error.
 */
export const LIMIT_ERROR_PREFIX = "LIMIT:";

/**
 * Ask Supabase to refresh the current session so we get a new access token.
 * The auth-state-change listener in AuthProvider picks up the new session
 * and writes it into useAuthStore — so a subsequent read of
 * `useAuthStore.getState().session.access_token` returns the fresh token.
 *
 * Returns the new access token, or null if no Supabase client is configured
 * or the refresh failed (no active refresh token, network error, etc).
 */
async function refreshAccessToken(): Promise<string | null> {
  const supabase = getSupabase();
  if (!supabase) return null;
  try {
    const { data, error } = await supabase.auth.refreshSession();
    if (error || !data.session) return null;
    return data.session.access_token;
  } catch {
    return null;
  }
}

/** Build the Authorization header from the current store session. Returns
 *  an empty record when there's no session — callers are anonymous in that
 *  case and the server applies the anon-tier rate limits. */
function authHeaders(): Record<string, string> {
  const session = useAuthStore.getState().session;
  if (!session?.access_token) return {};
  return { Authorization: `Bearer ${session.access_token}` };
}

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
  /** Server-side persistence echoes the message id assigned to the
   * streaming assistant row + the thread id (which may have been created
   * lazily server-side). The caller uses this to reconcile its in-memory
   * placeholder with the persisted row. */
  onMeta?: (meta: { threadId: string; assistantMessageId: string }) => void;
}

export async function streamChat(
  messages: ChatRequestMessage[],
  context: SelectionContext[],
  callbacks: ChatStreamCallbacks,
  options?: {
    tools?: AnthropicTool[];
    systemPrompt?: string;
    signal?: AbortSignal;
    /** Persistence context — required for the server to write the turn to
     * chat_threads / chat_messages / chat_message_deltas. Omit to use the
     * legacy in-memory-only path. */
    threadId?: string | null;
    documentId?: string | null;
    userMessageId?: string | null;
    parentMessageId?: string | null;
    /** Pre-generated assistant message id; the server uses it for the
     * persisted row so Realtime updates match the in-memory placeholder. */
    assistantMessageId?: string | null;
  },
): Promise<void> {
  const selectedParts = context.map((c) => ({
    partId: c.partId,
    partName: c.partName,
    geometryType: c.geometryType,
  }));

  try {
    const { apiUrl } = await import("@/lib/api-origin");
    const url = apiUrl("/api/chat");
    const requestBody = JSON.stringify({
      messages,
      context: { selectedParts },
      tools: options?.tools,
      systemPrompt: options?.systemPrompt,
      thread_id: options?.threadId ?? null,
      document_id: options?.documentId ?? null,
      user_message_id: options?.userMessageId ?? null,
      parent_message_id: options?.parentMessageId ?? null,
      assistant_message_id: options?.assistantMessageId ?? null,
    });

    // Attach the Supabase access token if the user is signed in (including
    // anonymous sessions), so the backend can scope persistence rows to
    // their auth.uid() and apply the right rate-limit tier.
    const post = (extraHeaders: Record<string, string>) =>
      fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...extraHeaders,
        },
        body: requestBody,
        signal: options?.signal,
      });

    let response = await post(authHeaders());

    // The server returns 401 with `error: "auth_invalid"` when a Bearer
    // token was sent but Supabase rejected it (typically: the access token
    // expired between the auto-refresh and this send). Refresh the session
    // once and retry the same request — without this, the user would see a
    // misleading "Free chat limit reached" banner because the server would
    // have routed the orphaned request through the anon IP rate limit.
    if (response.status === 401 && useAuthStore.getState().session) {
      const newToken = await refreshAccessToken();
      if (newToken) {
        response = await post({ Authorization: `Bearer ${newToken}` });
      }
    }

    if (response.status === 401) {
      callbacks.onError(
        "Sign-in session expired. Please sign in again to continue.",
      );
      callbacks.onFinish();
      return;
    }

    if (response.status === 429) {
      // Rate limit hit — pass the full JSON body through with a prefix so
      // the chat handler can distinguish this from a normal network error.
      const bodyText = await response.text();
      callbacks.onError(`${LIMIT_ERROR_PREFIX}${bodyText}`);
      callbacks.onFinish();
      return;
    }

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
      if (options?.signal?.aborted) {
        try { reader.cancel(); } catch { /* ignore */ }
        break;
      }
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
            case "meta":
              callbacks.onMeta?.({
                threadId: event.thread_id,
                assistantMessageId: event.assistant_message_id,
              });
              break;
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
    // AbortError fires when the caller cancels via signal — that's a
    // user-initiated stop, not a failure, so we don't surface it as an error.
    const isAbort =
      (err instanceof DOMException && err.name === "AbortError") ||
      options?.signal?.aborted === true;
    if (!isAbort) {
      callbacks.onError(err instanceof Error ? err.message : "Stream failed");
    }
    callbacks.onFinish();
  }
}
