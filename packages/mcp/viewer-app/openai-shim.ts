/**
 * ChatGPT Apps SDK host adapter.
 *
 * ChatGPT injects a `window.openai` bridge into widget iframes instead of
 * speaking the MCP Apps postMessage protocol. This shim exposes the
 * subset of the `App` class surface that main.ts uses, backed by
 * `window.openai`, so the same viewer bundle runs on both hosts:
 *
 *   MCP Apps host (Claude, Cursor) → `new App(...)` (ext-apps)
 *   ChatGPT                        → `createOpenAiShim()`
 *
 * Mapping:
 *   ontoolresult        ← openai.toolOutput (the tool's structuredContent),
 *                         synthesized into a ToolResultLike; refreshed on
 *                         `openai:set_globals`
 *   callServerTool      ← openai.callTool(name, args)  (tools must carry
 *                         `_meta["openai/widgetAccessible"]: true`)
 *   sendMessage         ← openai.sendFollowUpMessage({ prompt })
 *   openLink            ← openai.openExternal({ href })
 *   requestDisplayMode  ← openai.requestDisplayMode({ mode })
 *   host context        ← openai.theme / openai.displayMode
 *   updateModelContext  → unavailable (capability stays unset; the viewer
 *                         degrades to its local selection inspector)
 */

/** The slice of `window.openai` this shim consumes (Apps SDK). */
interface OpenAiBridge {
  toolInput?: unknown;
  toolOutput?: unknown;
  widgetState?: unknown;
  theme?: string;
  displayMode?: string;
  callTool?: (name: string, args: Record<string, unknown>) => Promise<unknown>;
  sendFollowUpMessage?: (opts: { prompt: string }) => Promise<unknown>;
  openExternal?: (opts: { href: string }) => unknown;
  requestDisplayMode?: (opts: { mode: string }) => Promise<{ mode?: string } | undefined>;
  setWidgetState?: (state: unknown) => Promise<unknown>;
}

declare global {
  interface Window {
    openai?: OpenAiBridge;
  }
}

/** True when running inside a ChatGPT widget iframe. */
export function isOpenAiHost(): boolean {
  return typeof window !== "undefined" && Boolean(window.openai);
}

interface ToolResultShape {
  content: Array<{ type: string; text?: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
}

/** Wrap a toolOutput global as the ToolResultLike main.ts consumes. */
function synthesizeResult(toolOutput: unknown): ToolResultShape {
  return {
    content: [],
    structuredContent:
      toolOutput && typeof toolOutput === "object"
        ? (toolOutput as Record<string, unknown>)
        : undefined,
    isError: false,
  };
}

/** Normalize callTool's return shape to a tool result. */
function normalizeToolResult(raw: unknown): unknown {
  if (raw && typeof raw === "object") {
    const obj = raw as Record<string, unknown>;
    if (Array.isArray(obj.content)) return obj;
    const inner = obj.result as Record<string, unknown> | undefined;
    if (inner && Array.isArray(inner.content)) return inner;
  }
  return raw ?? {};
}

export function createOpenAiShim() {
  const oai = window.openai!;
  let lastToolOutput: unknown;

  const shim = {
    // Handlers main.ts assigns — same contract as the App class.
    onhostcontextchanged: undefined as
      | ((params: { hostContext: Record<string, unknown> }) => void)
      | undefined,
    ontoolinput: undefined as (() => void) | undefined,
    ontoolresult: undefined as ((result: unknown) => void) | undefined,

    getHostContext(): Record<string, unknown> {
      return {
        theme: oai.theme === "light" ? "light" : "dark",
        displayMode: oai.displayMode ?? "inline",
        // ChatGPT supports pip — advertise it so the canvas can auto-dock as a
        // persistent side panel that updates across the conversation.
        availableDisplayModes: ["inline", "pip", "fullscreen"],
      };
    },

    getHostCapabilities(): Record<string, unknown> {
      return {
        // Ask button (ui/message ↔ sendFollowUpMessage)
        ...(oai.sendFollowUpMessage ? { message: {} } : {}),
        // No ChatGPT equivalent of ui/update-model-context — leave unset
        // so selection degrades to the local inspector.
      };
    },

    async connect(): Promise<void> {
      window.addEventListener("openai:set_globals", () => {
        shim.onhostcontextchanged?.({ hostContext: shim.getHostContext() });
        if (oai.toolOutput !== lastToolOutput && oai.toolOutput != null) {
          lastToolOutput = oai.toolOutput;
          shim.ontoolresult?.(synthesizeResult(oai.toolOutput));
        }
      });
      // Tool output may already be set when the widget hydrates.
      if (oai.toolOutput != null) {
        lastToolOutput = oai.toolOutput;
        queueMicrotask(() => shim.ontoolresult?.(synthesizeResult(oai.toolOutput)));
      }
    },

    async callServerTool(opts: {
      name: string;
      arguments: Record<string, unknown>;
    }): Promise<unknown> {
      if (!oai.callTool) {
        throw new Error("host does not allow widget tool calls");
      }
      return normalizeToolResult(await oai.callTool(opts.name, opts.arguments));
    },

    async sendMessage(opts: {
      role: string;
      content: Array<{ type: string; text: string }>;
    }): Promise<{ isError?: boolean }> {
      if (!oai.sendFollowUpMessage) return { isError: true };
      const prompt = opts.content
        .map((c) => (c.type === "text" ? c.text : ""))
        .join("\n")
        .trim();
      await oai.sendFollowUpMessage({ prompt });
      return {};
    },

    async updateModelContext(): Promise<void> {
      // Unreachable: getHostCapabilities never advertises it here.
    },

    async openLink(opts: { url: string }): Promise<void> {
      if (oai.openExternal) {
        oai.openExternal({ href: opts.url });
        return;
      }
      throw new Error("openExternal unavailable");
    },

    async requestDisplayMode(opts: { mode: string }): Promise<{ mode: string }> {
      if (!oai.requestDisplayMode) return { mode: "inline" };
      const granted = await oai.requestDisplayMode({ mode: opts.mode });
      return { mode: granted?.mode ?? "inline" };
    },
  };

  return shim;
}
