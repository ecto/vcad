/**
 * Single source of truth for one MCP tool: its advertised descriptor
 * (name/description/inputSchema), its handler, and the cross-cutting behavior
 * flags the server pipeline reads. Every tool module exports a `ToolDef[]`;
 * `server.ts` assembles them into the ListTools surface and a name→def dispatch
 * Map, so adding a tool touches only its module.
 *
 * The flags replace the parallel name-sets the server used to hand-sync
 * (geometry / doc-writer / mount / widget-callable / preview-fetch /
 * pure-JSON) and the per-pack `TOOL_PACKS` table (now `pack`).
 */

import type { Engine } from "@vcad/engine";
import type { AuthUser } from "../oauth.js";
import type {
  SessionStore,
  SessionEventStore,
  ShareStore,
} from "../session-store.js";
import type { FabricateStore } from "../fabricate/store.js";
import type { ToolResult } from "./tool-result.js";

/** Per-tool cross-cutting behavior, read by the server pipeline instead of the
 *  old parallel name-sets. */
export interface ToolBehavior {
  /** Creates or mutates a session Document → persist + append an event, and
   *  snapshot the prior state for `undo`. (Registry mutators derive this too:
   *  every dispatchable kernel tool except `read`.) */
  writesDoc: boolean;
  /** Produces or changes geometry → attach a preview handle so the inline 3D
   *  viewer can fetch/refresh it. (Every registry kernel tool is geometry.) */
  geometry: boolean;
  /** Begins a viewable session → carries the viewer UI template (`_meta`). */
  mount: boolean;
  /** The viewer iframe calls this itself (deep-link / ledger) → marked
   *  widget-accessible, but never mounts a template. */
  widgetCallable: boolean;
  /** App-only fetcher the viewer polls → hidden from the model (`app`
   *  visibility), widget-accessible, no template. */
  appOnly: boolean;
  /** The text body is a machine-parseable JSON document consumers `JSON.parse`
   *  verbatim → never append a handle block or slim it. */
  pureJson: boolean;
}

/** Everything a tool handler may need, threaded per connection from
 *  `createServer`. Handlers take only what they use. */
export interface ToolContext {
  engine: Engine;
  user: AuthUser | null;
  sessionStore: SessionStore;
  eventStore: SessionEventStore;
  fabricateStore: FabricateStore;
  shareStore: ShareStore;
  /**
   * Server-injected elicitation bridge (MCP URL-mode elicitation, SDK ≥1.29).
   * Injected by `createServer` when the transport can carry an in-band
   * elicitation round-trip; absent on stdio/legacy clients. `urlSupported()`
   * re-checks the client's `elicitation.url` capability at CALL time
   * (capabilities land only after initialize); `requestUrl` sends a
   * `mode:"url"` elicitation and resolves with the human's action. Tools must
   * treat this as an accelerator, never a dependency: every flow it fronts
   * (e.g. spend approval) keeps its out-of-band fallback, and a thrown
   * elicitation must never lose work already persisted.
   */
  elicit?: {
    urlSupported(): boolean;
    requestUrl(p: {
      message: string;
      url: string;
      elicitationId: string;
    }): Promise<{ action: "accept" | "decline" | "cancel" }>;
  };
}

/** A tool's implementation. `args` is the raw MCP argument object. */
export type ToolHandler = (
  args: Record<string, unknown>,
  ctx: ToolContext,
) => ToolResult | Promise<ToolResult>;

/** A single tool: advertised descriptor + handler + behavior. */
export interface ToolDef {
  name: string;
  /** Domain pack gating the tool via `VCAD_MCP_PACKS`; `null` = always-on
   *  core. */
  pack: string | null;
  description: string;
  /** JSON Schema for the tool's arguments (the `inputSchema` advertised in
   *  ListTools). */
  inputSchema: Record<string, unknown>;
  handler: ToolHandler;
  behavior: ToolBehavior;
  /** Human-readable display name advertised as the tool's `title`. Populated
   *  from `TOOL_METADATA` at assembly time (see `tool-metadata.ts`); the
   *  server asserts every advertised tool has one. */
  title?: string;
  /** MCP tool annotations (readOnlyHint, destructiveHint, openWorldHint, …).
   *  Advertised verbatim. Populated from `TOOL_METADATA` at assembly time. */
  annotations?: Record<string, unknown>;
  /** JSON Schema for the tool's structured result. Declared only on tools that
   *  return `structuredContent` on every success (per MCP spec). Populated
   *  from `TOOL_METADATA` at assembly time. */
  outputSchema?: Record<string, unknown>;
}

/** Terse constructor for a ToolDef's `behavior`, defaulting every flag off.
 *  Keeps module tool tables readable — most tools set one or two flags. */
export function behavior(flags: Partial<ToolBehavior> = {}): ToolBehavior {
  return {
    writesDoc: false,
    geometry: false,
    mount: false,
    widgetCallable: false,
    appOnly: false,
    pureJson: false,
    ...flags,
  };
}
