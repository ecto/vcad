/**
 * MCP `2026-07-28` ("modern") protocol support.
 *
 * The 2026-07-28 revision removes the `initialize` handshake, protocol-level
 * sessions, and the standalone GET stream: every request is self-describing
 * (protocol version, client identity, and client capabilities ride in
 * `params._meta`), and `server/discover` replaces `initialize` as the way a
 * client learns what a server speaks.
 *
 * The TypeScript SDK does not implement this revision yet (1.30.0's
 * `LATEST_PROTOCOL_VERSION` is still `2025-11-25`), so this module is a
 * *dual-era* front end: it answers modern requests itself and leaves legacy
 * (`initialize`-based) traffic on the SDK's transport untouched. The spec
 * explicitly sanctions this shape — "A dual-era server MAY serve both eras
 * concurrently on the same endpoint or process."
 *
 * How modern requests are served: rather than duplicating the ~2k lines of
 * tool dispatch in `server.ts`, a modern request is bridged onto a real SDK
 * `Server` over an in-process `InMemoryTransport` pair. The bridge performs
 * the legacy handshake on the client side of the pair, forwards the request,
 * then re-shapes the reply into a modern result (`resultType`, cache hints,
 * `_meta['io.modelcontextprotocol/serverInfo']`). vcad's HTTP entry already
 * builds a fresh `Server` per request, so the bridge adds no server
 * construction that wasn't happening anyway.
 *
 * Known gaps, stated rather than papered over:
 *   - Responses are always `application/json`. Notifications emitted by a tool
 *     (`notifications/progress`, `notifications/message`) are dropped instead
 *     of being streamed on the request's SSE response stream.
 *   - `subscriptions/listen` is not implemented; this server is stateless and
 *     has no per-connection subscription state to feed it. Reported honestly
 *     as method-not-found rather than acknowledged and left silent.
 *   - MRTR (`InputRequiredResult`) is not implemented. A server-initiated
 *     request raised inside the bridge (URL-mode elicitation) is refused, and
 *     `server.ts` already degrades that to `{action:"cancel"}`.
 *   - The Tasks extension (`io.modelcontextprotocol/tasks`) is not advertised.
 */

import type { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import type { JSONRPCMessage } from "@modelcontextprotocol/sdk/types.js";

/** The protocol revision this module implements. */
export const PROTOCOL_2026 = "2026-07-28";

/**
 * Version spoken on the in-process bridge to the SDK `Server`. Internal
 * plumbing only — never advertised to a client.
 */
const BRIDGE_VERSION = "2025-11-25";

/**
 * Every revision this server can serve: the modern one via this module, the
 * legacy ones via the SDK's own `initialize` path. Ordered newest-first, which
 * is the order a client should prefer when picking from `supported`.
 */
export const SUPPORTED_PROTOCOL_VERSIONS: readonly string[] = [
  PROTOCOL_2026,
  "2025-11-25",
  "2025-06-18",
  "2025-03-26",
  "2024-11-05",
];

// ── `_meta` keys defined by the revision ──────────────────────────

export const META_PROTOCOL_VERSION = "io.modelcontextprotocol/protocolVersion";
export const META_CLIENT_CAPABILITIES =
  "io.modelcontextprotocol/clientCapabilities";
export const META_CLIENT_INFO = "io.modelcontextprotocol/clientInfo";
export const META_SERVER_INFO = "io.modelcontextprotocol/serverInfo";

// ── Error codes ──────────────────────────────────────────────────
// -32020..-32099 is the range the spec reserves for itself; -32000..-32019
// stays implementation-defined (existing SDK usage is grandfathered).

/** Headers disagree with the body, or a required header is missing. */
export const ERR_HEADER_MISMATCH = -32020;
/** A capability the request requires was not declared by the client. */
export const ERR_MISSING_REQUIRED_CLIENT_CAPABILITY = -32021;
/** The requested protocol version is not one this server serves. */
export const ERR_UNSUPPORTED_PROTOCOL_VERSION = -32022;
const ERR_METHOD_NOT_FOUND = -32601;
const ERR_INVALID_PARAMS = -32602;
const ERR_INTERNAL = -32603;
/** Pre-2026 code for "resource not found"; renumbered to Invalid Params. */
const LEGACY_RESOURCE_NOT_FOUND = -32002;

/**
 * Freshness hint for list-shaped results, in ms. Deliberately short: the tool
 * surface is mutable at runtime (`set_tool_packs` re-advertises it), so a long
 * TTL would let a client serve a stale catalog for minutes after a pack flip.
 */
const LIST_TTL_MS = 60_000;
/** `server/discover` changes only on deploy; safe to cache longer. */
const DISCOVER_TTL_MS = 300_000;
/**
 * Always `private`: results vary with the caller's bearer token (tool packs,
 * document scope, Fabricate state), so a shared intermediary must not reuse
 * one caller's response for another.
 */
const CACHE_SCOPE = "private" as const;

/** Methods whose results carry `ttlMs` / `cacheScope` per `CacheableResult`. */
const CACHEABLE_METHODS = new Map<string, number>([
  ["server/discover", DISCOVER_TTL_MS],
  ["tools/list", LIST_TTL_MS],
  ["prompts/list", LIST_TTL_MS],
  ["resources/list", LIST_TTL_MS],
  ["resources/templates/list", LIST_TTL_MS],
  ["resources/read", LIST_TTL_MS],
]);

/** Methods forwarded verbatim to the bridged SDK server. */
const FORWARDED_METHODS = new Set<string>([
  "tools/list",
  "tools/call",
  "resources/list",
  "resources/templates/list",
  "resources/read",
  "prompts/list",
  "prompts/get",
  "completion/complete",
]);

/** `Mcp-Name` mirrors `params.name` for these; `params.uri` for resources. */
const NAME_HEADER_METHODS = new Set(["tools/call", "prompts/get"]);

// ── Types ────────────────────────────────────────────────────────

interface JsonRpcRequest {
  jsonrpc: "2.0";
  id?: string | number | null;
  method?: string;
  params?: Record<string, unknown>;
}

/** Case-insensitive header bag. Absent for stdio, where there are no headers. */
export type HeaderBag = Record<string, string | string[] | undefined>;

export interface ModernResponse {
  /** HTTP status the caller should use. `null` body means "no body" (202). */
  status: number;
  body: unknown | null;
}

export interface ModernOptions {
  /** Builds the SDK server the request is bridged onto. */
  createServer: () => Promise<Server>;
  /** Request headers, when the transport has them (HTTP). Omit for stdio. */
  headers?: HeaderBag;
}

// ── Detection ────────────────────────────────────────────────────

function header(headers: HeaderBag | undefined, name: string): string | undefined {
  if (!headers) return undefined;
  const lower = name.toLowerCase();
  for (const [k, v] of Object.entries(headers)) {
    if (k.toLowerCase() !== lower) continue;
    return Array.isArray(v) ? v[0] : v;
  }
  return undefined;
}

function metaOf(msg: unknown): Record<string, unknown> | undefined {
  const params = (msg as JsonRpcRequest | undefined)?.params;
  const meta = params?._meta;
  return meta && typeof meta === "object"
    ? (meta as Record<string, unknown>)
    : undefined;
}

/**
 * True when a message should be served by this module rather than the SDK.
 *
 * Two independent signals, either of which is conclusive: the body declares a
 * protocol version in `_meta` (only modern clients do this), or the method is
 * `server/discover` (which exists only in the modern revision — a legacy
 * client has no reason to send it). The `MCP-Protocol-Version` header alone is
 * NOT a signal: `2025-06-18` and later legacy clients send it too.
 */
export function isModernMessage(
  msg: unknown,
  headers?: HeaderBag,
): boolean {
  const first = Array.isArray(msg) ? msg[0] : msg;
  if (!first || typeof first !== "object") return false;
  const method = (first as JsonRpcRequest).method;
  if (method === "server/discover") return true;
  const declared = metaOf(first)?.[META_PROTOCOL_VERSION];
  if (typeof declared === "string") return true;
  // A `Mcp-Method` header is required on modern POSTs and defined nowhere
  // else, so treat it as a fallback signal for a body we couldn't read.
  return header(headers, "mcp-method") !== undefined;
}

// ── Header value encoding ────────────────────────────────────────

const BASE64_SENTINEL_PREFIX = "=?base64?";
const BASE64_SENTINEL_SUFFIX = "?=";

/** Decode the `=?base64?...?=` sentinel form used for non-ASCII header values. */
function decodeHeaderValue(value: string): string {
  if (
    !value.startsWith(BASE64_SENTINEL_PREFIX) ||
    !value.endsWith(BASE64_SENTINEL_SUFFIX) ||
    value.length < BASE64_SENTINEL_PREFIX.length + BASE64_SENTINEL_SUFFIX.length
  ) {
    return value;
  }
  const encoded = value.slice(
    BASE64_SENTINEL_PREFIX.length,
    value.length - BASE64_SENTINEL_SUFFIX.length,
  );
  try {
    return Buffer.from(encoded, "base64").toString("utf-8");
  } catch {
    return value;
  }
}

// ── Response helpers ─────────────────────────────────────────────

function errorResponse(
  id: string | number | null | undefined,
  code: number,
  message: string,
  data?: unknown,
): unknown {
  return {
    jsonrpc: "2.0",
    id: id ?? null,
    error: data === undefined ? { code, message } : { code, message, data },
  };
}

function unsupportedVersion(
  id: string | number | null | undefined,
  requested: string,
): ModernResponse {
  return {
    status: 400,
    body: errorResponse(
      id,
      ERR_UNSUPPORTED_PROTOCOL_VERSION,
      "Unsupported protocol version",
      { supported: SUPPORTED_PROTOCOL_VERSIONS, requested },
    ),
  };
}

function headerMismatch(
  id: string | number | null | undefined,
  message: string,
): ModernResponse {
  return {
    status: 400,
    body: errorResponse(id, ERR_HEADER_MISMATCH, `Header mismatch: ${message}`),
  };
}

// ── The in-process bridge ────────────────────────────────────────

interface BridgeReply {
  result?: Record<string, unknown>;
  error?: { code: number; message: string; data?: unknown };
}

interface Bridge {
  /** Send a JSON-RPC request and await its response message. */
  rpc(method: string, params?: Record<string, unknown>): Promise<BridgeReply>;
  /** The bridged server's own `initialize` result — capabilities, identity,
   *  and instructions, which `server/discover` reshapes and every modern
   *  result stamps into `_meta`. */
  handshake: {
    capabilities?: unknown;
    serverInfo?: { name?: string; version?: string };
    instructions?: string;
  };
  close(): Promise<void>;
}

/**
 * Stand up an SDK `Server` on one end of an in-memory transport pair and drive
 * it from the other end with raw JSON-RPC. The legacy `initialize` handshake
 * happens here, invisible to the modern client.
 */
async function openBridge(
  createServer: () => Promise<Server>,
  clientInfo: { name: string; version: string },
  clientCapabilities: Record<string, unknown>,
): Promise<Bridge> {
  const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();

  let nextId = 1;
  const pending = new Map<
    string | number,
    (msg: Record<string, unknown>) => void
  >();

  clientSide.onmessage = (msg: JSONRPCMessage) => {
    const m = msg as unknown as Record<string, unknown>;
    const id = m.id as string | number | undefined;
    if (id !== undefined && ("result" in m || "error" in m)) {
      pending.get(id)?.(m);
      pending.delete(id);
      return;
    }
    if (id !== undefined && typeof m.method === "string") {
      // A server-initiated request (URL-mode elicitation). Under 2026-07-28
      // these become MRTR input requests, which this shim does not implement —
      // refuse so the caller's `catch` degrades it to a dismissed prompt
      // rather than hanging the request.
      void clientSide.send({
        jsonrpc: "2.0",
        id,
        error: {
          code: ERR_METHOD_NOT_FOUND,
          message:
            "Server-initiated requests are not available under protocol 2026-07-28 (MRTR not implemented)",
        },
      } as unknown as JSONRPCMessage);
    }
    // Notifications (progress, logging) are dropped: this shim answers with a
    // single JSON object, not an SSE response stream.
  };

  await clientSide.start();
  const server = await createServer();
  await server.connect(serverSide);

  const rpc: Bridge["rpc"] = (method, params) => {
    const id = nextId++;
    return new Promise((resolve) => {
      pending.set(id, (m) =>
        resolve(
          m as {
            result?: Record<string, unknown>;
            error?: { code: number; message: string; data?: unknown };
          },
        ),
      );
      void clientSide.send({
        jsonrpc: "2.0",
        id,
        method,
        params: params ?? {},
      } as unknown as JSONRPCMessage);
    });
  };

  // Legacy handshake on the bridge. The modern client never sees it.
  const init = await rpc("initialize", {
    protocolVersion: BRIDGE_VERSION,
    capabilities: clientCapabilities,
    clientInfo,
  });
  await clientSide.send({
    jsonrpc: "2.0",
    method: "notifications/initialized",
  } as unknown as JSONRPCMessage);

  return {
    rpc,
    handshake: (init.result ?? {}) as Bridge["handshake"],
    close: async () => {
      pending.clear();
      await server.close().catch(() => {});
      await clientSide.close().catch(() => {});
    },
  };
}

// ── Entry point ──────────────────────────────────────────────────

/**
 * Serve one modern JSON-RPC message. Returns the HTTP status and body the
 * caller should write; `body: null` means "write no body" (a notification
 * acknowledged with 202).
 */
export async function handleModernRequest(
  msg: unknown,
  opts: ModernOptions,
): Promise<ModernResponse> {
  if (Array.isArray(msg)) {
    return {
      status: 400,
      body: errorResponse(
        null,
        ERR_INVALID_PARAMS,
        "Batched requests are not supported; send one JSON-RPC message per POST",
      ),
    };
  }
  if (!msg || typeof msg !== "object") {
    return {
      status: 400,
      body: errorResponse(null, ERR_INVALID_PARAMS, "Malformed JSON-RPC message"),
    };
  }

  const req = msg as JsonRpcRequest;
  const id = req.id;
  const method = req.method;
  const params = (req.params ?? {}) as Record<string, unknown>;
  const meta = metaOf(req) ?? {};

  // A notification carries no id: acknowledge and do nothing. This revision
  // defines no client-to-server notifications over Streamable HTTP.
  if (id === undefined || id === null) return { status: 202, body: null };

  if (typeof method !== "string") {
    return {
      status: 400,
      body: errorResponse(id, ERR_INVALID_PARAMS, "Missing JSON-RPC method"),
    };
  }

  // ── Version check ───────────────────────────────────────────────
  const bodyVersion = meta[META_PROTOCOL_VERSION];
  const headerVersion = header(opts.headers, "mcp-protocol-version");
  if (
    typeof bodyVersion === "string" &&
    headerVersion !== undefined &&
    headerVersion !== bodyVersion
  ) {
    return headerMismatch(
      id,
      `MCP-Protocol-Version header value '${headerVersion}' does not match body value '${bodyVersion}'`,
    );
  }
  const version =
    typeof bodyVersion === "string" ? bodyVersion : headerVersion;
  // `server/discover` is the version-negotiation probe itself, so a client may
  // send it without having picked a version yet — but a version it *does* name
  // still has to be one we serve. Every other method must name one.
  if (version === undefined) {
    if (method !== "server/discover") {
      return unsupportedVersion(id, "(unspecified)");
    }
  } else if (version !== PROTOCOL_2026) {
    return unsupportedVersion(id, version);
  }

  // ── Header/body agreement ───────────────────────────────────────
  // Only enforced on transports that actually carry headers. A load balancer
  // routing on `Mcp-Method` while the server dispatches on the body is exactly
  // the split-brain this check exists to prevent.
  if (opts.headers) {
    const methodHeader = header(opts.headers, "mcp-method");
    if (methodHeader === undefined) {
      return headerMismatch(id, "required header Mcp-Method is missing");
    }
    if (methodHeader !== method) {
      return headerMismatch(
        id,
        `Mcp-Method header value '${methodHeader}' does not match body value '${method}'`,
      );
    }

    const expectedName = NAME_HEADER_METHODS.has(method)
      ? params.name
      : method === "resources/read"
        ? params.uri
        : undefined;
    if (typeof expectedName === "string") {
      const nameHeader = header(opts.headers, "mcp-name");
      if (nameHeader === undefined) {
        return headerMismatch(id, "required header Mcp-Name is missing");
      }
      if (decodeHeaderValue(nameHeader) !== expectedName) {
        return headerMismatch(
          id,
          `Mcp-Name header value does not match body value '${expectedName}'`,
        );
      }
    }
  }

  // ── Methods this revision removed or that we do not implement ───
  // `initialize` gets a bespoke message: a legacy client has no fall-forward
  // mechanism, so per the spec this error may be the only diagnostic a user
  // ever sees — it must name the versions this server does serve. (Reaching
  // here at all means the client sent modern `_meta` alongside a legacy
  // handshake; a plain legacy `initialize` is routed to the SDK, not here.)
  if (method === "initialize") {
    return {
      status: 404,
      body: errorResponse(
        id,
        ERR_METHOD_NOT_FOUND,
        `'initialize' was removed in protocol ${PROTOCOL_2026}; use 'server/discover'. This server supports: ${SUPPORTED_PROTOCOL_VERSIONS.join(", ")}`,
      ),
    };
  }
  if (method === "subscriptions/listen") {
    return {
      status: 404,
      body: errorResponse(
        id,
        ERR_METHOD_NOT_FOUND,
        "subscriptions/listen is not implemented: this server is stateless and holds no per-connection subscription state",
      ),
    };
  }
  if (method !== "server/discover" && !FORWARDED_METHODS.has(method)) {
    return {
      status: 404,
      body: errorResponse(id, ERR_METHOD_NOT_FOUND, `Unknown method '${method}'`),
    };
  }

  // ── Bridge to the SDK server ────────────────────────────────────
  const rawClientInfo = meta[META_CLIENT_INFO];
  const clientInfo =
    rawClientInfo && typeof rawClientInfo === "object"
      ? (rawClientInfo as { name?: string; version?: string })
      : {};
  const rawCaps = meta[META_CLIENT_CAPABILITIES];
  const clientCapabilities =
    rawCaps && typeof rawCaps === "object"
      ? (rawCaps as Record<string, unknown>)
      : {};

  let bridge: Bridge | undefined;
  try {
    bridge = await openBridge(
      opts.createServer,
      {
        name: typeof clientInfo.name === "string" ? clientInfo.name : "unknown",
        version:
          typeof clientInfo.version === "string" ? clientInfo.version : "0.0.0",
      },
      clientCapabilities,
    );

    if (method === "server/discover") {
      const info = bridge.handshake;
      const result: Record<string, unknown> = {
        resultType: "complete",
        supportedVersions: SUPPORTED_PROTOCOL_VERSIONS,
        capabilities: info.capabilities ?? {},
        _meta: { [META_SERVER_INFO]: info.serverInfo ?? {} },
      };
      if (info.instructions) result.instructions = info.instructions;
      decorateCache(result, "server/discover");
      return { status: 200, body: { jsonrpc: "2.0", id, result } };
    }

    // Forward with the modern-only `_meta` keys stripped: the SDK server has
    // no use for them and passing an unknown `_meta` through tool arguments
    // is noise in the dispatch layer.
    const forwarded = stripModernMeta(params);
    const reply = await bridge.rpc(method, forwarded);

    if (reply.error) {
      const code =
        reply.error.code === LEGACY_RESOURCE_NOT_FOUND
          ? ERR_INVALID_PARAMS
          : reply.error.code;
      return {
        status: code === ERR_METHOD_NOT_FOUND ? 404 : 200,
        body: errorResponse(id, code, reply.error.message, reply.error.data),
      };
    }

    const result = { ...(reply.result ?? {}) } as Record<string, unknown>;
    result.resultType = "complete";
    const resultMeta =
      result._meta && typeof result._meta === "object"
        ? { ...(result._meta as Record<string, unknown>) }
        : {};
    resultMeta[META_SERVER_INFO] = bridge.handshake.serverInfo ?? {
      name: "vcad",
      version: "0.0.0",
    };
    result._meta = resultMeta;
    decorateCache(result, method);

    return { status: 200, body: { jsonrpc: "2.0", id, result } };
  } catch (err) {
    return {
      status: 500,
      body: errorResponse(
        id,
        ERR_INTERNAL,
        err instanceof Error ? err.message : String(err),
      ),
    };
  } finally {
    await bridge?.close();
  }
}

/** Attach `ttlMs` / `cacheScope` to results the spec makes cacheable. */
function decorateCache(result: Record<string, unknown>, method: string): void {
  const ttl = CACHEABLE_METHODS.get(method);
  if (ttl === undefined) return;
  result.ttlMs = ttl;
  result.cacheScope = CACHE_SCOPE;
}

/** Drop the per-request modern `_meta` keys before forwarding to the SDK. */
function stripModernMeta(
  params: Record<string, unknown>,
): Record<string, unknown> {
  const meta = params._meta;
  if (!meta || typeof meta !== "object") return params;
  const rest = { ...(meta as Record<string, unknown>) };
  delete rest[META_PROTOCOL_VERSION];
  delete rest[META_CLIENT_CAPABILITIES];
  delete rest[META_CLIENT_INFO];
  const out = { ...params };
  if (Object.keys(rest).length === 0) delete out._meta;
  else out._meta = rest;
  return out;
}

