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
 * Beyond plain request/response, this module implements:
 *   - MRTR (SEP-2322): a server-initiated request raised mid-call (URL-mode
 *     elicitation) becomes an `InputRequiredResult`; the client retries the
 *     original call with `inputResponses` and the bridge answers the re-raised
 *     request from them. Deterministic `input_N` keys make the retry line up
 *     without any `requestState`.
 *   - The Tasks extension (`io.modelcontextprotocol/tasks`, SEP-2663): when a
 *     client declares it, calls to known long-running tools return a
 *     `CreateTaskResult` immediately and finish in the background; clients
 *     poll `tasks/get`, feed elicitations via `tasks/update`, and cancel
 *     cooperatively via `tasks/cancel`. Tasks live in process memory with a
 *     TTL — durable for the life of the stdio process or warm HTTP instance,
 *     which is exactly the durability vcad sessions already have.
 *   - `subscriptions/listen`: a long-lived notification stream serving
 *     `toolsListChanged` (fired when `set_tool_packs` re-shapes the surface)
 *     and `notifications/tasks` for subscribed task ids.
 *   - Request-scoped notification forwarding: `notifications/progress` (and
 *     `notifications/message`, gated on the request's `logLevel` `_meta` key
 *     as the revision requires) are handed to the transport for SSE delivery.
 */

import { EventEmitter } from "node:events";
import { randomUUID } from "node:crypto";
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
export const META_LOG_LEVEL = "io.modelcontextprotocol/logLevel";
export const META_SUBSCRIPTION_ID = "io.modelcontextprotocol/subscriptionId";

/** Extension identifier for the MCP Tasks extension. */
export const TASKS_EXTENSION = "io.modelcontextprotocol/tasks";

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

/** MRTR is allowed only on these client requests (spec §Supported Requests). */
const MRTR_METHODS = new Set(["tools/call", "prompts/get", "resources/read"]);

/**
 * Tools eligible to return a Tasks-extension handle when the client declares
 * `io.modelcontextprotocol/tasks`. These are the calls that routinely run for
 * tens of seconds to minutes — routing, field solvers, optimization, video —
 * where a durable handle beats holding an HTTP response open. Everything else
 * stays synchronous even for task-capable clients: a sub-second `inspect_cad`
 * gains nothing from a poll loop.
 */
const LONG_RUNNING_TOOLS = new Set<string>([
  "route_nets",
  "route_diff_pair",
  "length_match_traces",
  "fab_prep",
  "fix_drc",
  "topology_optimize",
  "optimize_electrodes",
  "export_video",
  "render_sequence",
  "simulate_flow",
  "simulate_em",
  "simulate_photonics",
  "simulate_charged_particles",
  "simulate_neutron_shield",
  "simulate_lattice_gauge",
  "md_run",
  "minimize_energy",
  "homogenize_material",
  "design_material",
]);

/** How long a finished (or abandoned) task is retrievable, per the Task TTL. */
const TASK_TTL_MS = 30 * 60_000;
/** Suggested poll cadence for `tasks/get`. */
const TASK_POLL_INTERVAL_MS = 1_000;

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
  /**
   * Receives request-scoped notifications (`notifications/progress`,
   * `notifications/message`) as they flow from the bridged server, for
   * delivery on the request's SSE response stream (HTTP) or the shared
   * channel (stdio). When omitted, notifications are dropped and the reply
   * is still complete — streaming is an enhancement, not a dependency.
   */
  onNotification?: (notification: Record<string, unknown>) => void;
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

/** Modern-only methods this module answers without a body version signal. */
const MODERN_ONLY_METHODS = new Set([
  "server/discover",
  "subscriptions/listen",
  "tasks/get",
  "tasks/update",
  "tasks/cancel",
]);

/**
 * True when a message should be served by this module rather than the SDK.
 *
 * Two independent signals, either of which is conclusive: the body declares a
 * protocol version in `_meta` (only modern clients do this), or the method
 * exists only in the modern revision (`server/discover`, `subscriptions/
 * listen`, `tasks/*` — a legacy client has no reason to send any of them).
 * The `MCP-Protocol-Version` header alone is NOT a signal: `2025-06-18` and
 * later legacy clients send it too.
 */
export function isModernMessage(
  msg: unknown,
  headers?: HeaderBag,
): boolean {
  const first = Array.isArray(msg) ? msg[0] : msg;
  if (!first || typeof first !== "object") return false;
  const method = (first as JsonRpcRequest).method;
  if (typeof method === "string" && MODERN_ONLY_METHODS.has(method)) return true;
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

// ── Process-level event bus ──────────────────────────────────────
// Feeds `subscriptions/listen` streams. Process-scoped on purpose: the tool
// surface (pack flips) and the task store are both process state, so their
// change events are too.

const bus = new EventEmitter();
bus.setMaxListeners(100);
const EVT_TOOLS_CHANGED = "tools_list_changed";
const EVT_TASK = "task";

// ── Task store (Tasks extension) ─────────────────────────────────

type TaskStatus =
  | "working"
  | "input_required"
  | "completed"
  | "failed"
  | "cancelled";

interface TaskRecord {
  taskId: string;
  status: TaskStatus;
  statusMessage?: string;
  createdAt: string;
  lastUpdatedAt: string;
  expiresAt: number;
  toolName: string;
  /** Outstanding MRTR-style requests while `input_required`. */
  inputRequests: Record<string, Record<string, unknown>>;
  /** Resolvers for elicitations awaiting a `tasks/update`. */
  pendingInputs: Map<string, (response: Record<string, unknown>) => void>;
  result?: Record<string, unknown>;
  error?: { code: number; message: string; data?: unknown };
  /** Cooperative-cancel flag read by the elicitation path. */
  cancelRequested: boolean;
  /** Tears down the background bridge on cancel/expiry. */
  dispose?: () => void;
}

const tasks = new Map<string, TaskRecord>();

function pruneTasks(): void {
  const now = Date.now();
  for (const [id, t] of tasks) {
    if (t.expiresAt <= now) {
      t.dispose?.();
      tasks.delete(id);
    }
  }
}

function touchTask(t: TaskRecord, status?: TaskStatus, message?: string): void {
  if (status) t.status = status;
  if (message !== undefined) t.statusMessage = message;
  t.lastUpdatedAt = new Date().toISOString();
  bus.emit(EVT_TASK, detailedTask(t));
}

/** The wire shape of a task: base fields plus status-specific ones inlined. */
function detailedTask(t: TaskRecord): Record<string, unknown> {
  const base: Record<string, unknown> = {
    taskId: t.taskId,
    status: t.status,
    createdAt: t.createdAt,
    lastUpdatedAt: t.lastUpdatedAt,
    ttlMs: TASK_TTL_MS,
    pollIntervalMs: TASK_POLL_INTERVAL_MS,
  };
  if (t.statusMessage !== undefined) base.statusMessage = t.statusMessage;
  if (t.status === "input_required") base.inputRequests = t.inputRequests;
  if (t.status === "completed") base.result = t.result ?? {};
  if (t.status === "failed") base.error = t.error ?? { code: ERR_INTERNAL, message: "unknown" };
  return base;
}

// ── The in-process bridge ────────────────────────────────────────

interface BridgeReply {
  result?: Record<string, unknown>;
  error?: { code: number; message: string; data?: unknown };
}

interface BridgeHooks {
  /** Request-scoped notifications from the bridged server. */
  onNotification?: (notification: Record<string, unknown>) => void;
  /**
   * A server-initiated request (elicitation) raised mid-call. Return the
   * result to answer it with; the bridge sends it back to the server.
   */
  onServerRequest: (
    method: string,
    params: Record<string, unknown>,
  ) => Promise<Record<string, unknown>>;
}

interface Bridge {
  /** Send a JSON-RPC request and await its response message. */
  rpc(method: string, params?: Record<string, unknown>): Promise<BridgeReply>;
  /** The bridged server's own `initialize` result — capabilities, identity,
   *  and instructions, which `server/discover` reshapes and every modern
   *  result stamps into `_meta`. */
  handshake: {
    capabilities?: Record<string, unknown>;
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
  hooks: BridgeHooks,
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
      // Server-initiated request (URL-mode elicitation). Route through the
      // hook: MRTR answers it from the retry's inputResponses (or captures it
      // and cancels), a task parks it as `input_required`.
      void hooks
        .onServerRequest(
          m.method,
          (m.params ?? {}) as Record<string, unknown>,
        )
        .then((result) =>
          clientSide.send({ jsonrpc: "2.0", id, result } as unknown as JSONRPCMessage),
        )
        .catch(() =>
          clientSide.send({
            jsonrpc: "2.0",
            id,
            error: { code: ERR_INTERNAL, message: "input bridge failed" },
          } as unknown as JSONRPCMessage),
        );
      return;
    }
    if (typeof m.method === "string" && id === undefined) {
      hooks.onNotification?.(m);
    }
  };

  await clientSide.start();
  const server = await createServer();
  await server.connect(serverSide);

  const rpc: Bridge["rpc"] = (method, params) => {
    const id = nextId++;
    return new Promise((resolve) => {
      pending.set(id, (m) => resolve(m as BridgeReply));
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

  // ── Task polling / input / cancel ───────────────────────────────
  if (method === "tasks/get" || method === "tasks/update" || method === "tasks/cancel") {
    return handleTaskMethod(id, method, params);
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
  if (method !== "server/discover" && !FORWARDED_METHODS.has(method)) {
    return {
      status: 404,
      body: errorResponse(id, ERR_METHOD_NOT_FOUND, `Unknown method '${method}'`),
    };
  }

  // ── Client identity, capabilities, and MRTR retry payload ───────
  const rawClientInfo = meta[META_CLIENT_INFO];
  const infoObj =
    rawClientInfo && typeof rawClientInfo === "object"
      ? (rawClientInfo as { name?: string; version?: string })
      : {};
  const clientInfo = {
    name: typeof infoObj.name === "string" ? infoObj.name : "unknown",
    version: typeof infoObj.version === "string" ? infoObj.version : "0.0.0",
  };
  const rawCaps = meta[META_CLIENT_CAPABILITIES];
  const clientCapabilities =
    rawCaps && typeof rawCaps === "object"
      ? (rawCaps as Record<string, unknown>)
      : {};
  const wantsLogs = typeof meta[META_LOG_LEVEL] === "string";

  const inputResponses =
    params.inputResponses && typeof params.inputResponses === "object"
      ? (params.inputResponses as Record<string, Record<string, unknown>>)
      : {};

  // Tasks: client declared the extension AND the tool is one that earns a
  // durable handle. The server decides per request; never return a task to a
  // client that did not opt in.
  const extCaps = (clientCapabilities.extensions ?? {}) as Record<string, unknown>;
  const asTask =
    method === "tools/call" &&
    TASKS_EXTENSION in extCaps &&
    typeof params.name === "string" &&
    LONG_RUNNING_TOOLS.has(params.name);

  // ── MRTR bookkeeping ────────────────────────────────────────────
  // Elicitations are keyed `input_1`, `input_2`, … in the order the tool
  // raises them. Re-running the tool on retry replays the same sequence, so
  // the keys line up without any server-side state or `requestState` blob.
  let inputSeq = 0;
  const capturedInputRequests: Record<string, Record<string, unknown>> = {};

  const onServerRequest: BridgeHooks["onServerRequest"] = async (
    reqMethod,
    reqParams,
  ) => {
    if (reqMethod !== "elicitation/create") {
      // Roots and sampling are deprecated features vcad never initiates.
      throw new Error(`unsupported server-initiated request ${reqMethod}`);
    }
    // Once the bridge is owned by a background task, elicitations park the
    // task as `input_required` instead of terminating the request (MRTR).
    if (bridgeRef) {
      const parked = taskElicitation(bridgeRef, reqParams);
      if (parked) return parked;
    }
    const key = `input_${++inputSeq}`;
    const provided = inputResponses[key];
    if (provided) return provided;
    if (!MRTR_METHODS.has(method)) return { action: "cancel" };
    // First sight of this elicitation: capture it for the InputRequiredResult
    // and answer the in-flight tool with a cancel so it winds down; its result
    // is discarded in favor of the input_required reply below. The spec's
    // basic workflow — the initial request terminates once input is needed.
    const { elicitationId: _drop, ...cleanParams } = reqParams;
    capturedInputRequests[key] = {
      method: "elicitation/create",
      params: cleanParams,
    };
    return { action: "cancel" };
  };

  let bridge: Bridge | undefined;
  // Stable reference to the opened bridge for the elicitation hook — survives
  // the ownership handoff (`bridge = undefined`) when a task takes over.
  let bridgeRef: Bridge | undefined;
  try {
    bridge = bridgeRef = await openBridge(opts.createServer, clientInfo, clientCapabilities, {
      onNotification: (n) => {
        const nm = n.method as string;
        // The revision removed the log-level RPC: notifications/message flows
        // only when this request carried a logLevel in `_meta`.
        if (nm === "notifications/message" && !wantsLogs) return;
        opts.onNotification?.(n);
      },
      onServerRequest,
    });

    if (method === "server/discover") {
      const info = bridge.handshake;
      const capabilities = { ...(info.capabilities ?? {}) } as Record<string, unknown>;
      // Advertise the Tasks extension alongside whatever the bridged server
      // declared (the MCP Apps UI extension rides in from server.ts).
      capabilities.extensions = {
        ...((capabilities.extensions ?? {}) as Record<string, unknown>),
        [TASKS_EXTENSION]: {},
      };
      const result: Record<string, unknown> = {
        resultType: "complete",
        supportedVersions: SUPPORTED_PROTOCOL_VERSIONS,
        capabilities,
        _meta: { [META_SERVER_INFO]: info.serverInfo ?? {} },
      };
      if (info.instructions) result.instructions = info.instructions;
      decorateCache(result, "server/discover");
      return { status: 200, body: { jsonrpc: "2.0", id, result } };
    }

    const serverInfo = bridge.handshake.serverInfo ?? {
      name: "vcad",
      version: "0.0.0",
    };
    const forwarded = stripModernFields(params);

    // ── Tasks path: return the handle now, finish in the background ──
    if (asTask) {
      pruneTasks();
      const record = createTaskRecord(params.name as string);
      const owned = bridge; // background closure owns the bridge from here
      bridge = undefined;
      runTaskInBackground(record, owned, method, forwarded, inputResponses);
      const result: Record<string, unknown> = {
        resultType: "task",
        ...detailedTask(record),
        _meta: { [META_SERVER_INFO]: serverInfo },
      };
      return { status: 200, body: { jsonrpc: "2.0", id, result } };
    }

    // ── Synchronous path ────────────────────────────────────────────
    const reply = await bridge.rpc(method, forwarded);

    // MRTR: the call raised input needs the retry must satisfy.
    if (Object.keys(capturedInputRequests).length > 0) {
      const result: Record<string, unknown> = {
        resultType: "input_required",
        inputRequests: capturedInputRequests,
        _meta: { [META_SERVER_INFO]: serverInfo },
      };
      return { status: 200, body: { jsonrpc: "2.0", id, result } };
    }

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
    resultMeta[META_SERVER_INFO] = serverInfo;
    result._meta = resultMeta;
    decorateCache(result, method);

    // The one mutation that re-shapes the tool surface at runtime. Legacy
    // connections learn via the SDK's own list_changed notification; modern
    // listeners learn through the process bus feeding subscriptions/listen.
    if (method === "tools/call" && params.name === "set_tool_packs") {
      bus.emit(EVT_TOOLS_CHANGED);
    }

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

/**
 * Drop the per-request modern `_meta` keys and MRTR retry fields before
 * forwarding to the SDK: the bridged server has no use for them, and
 * `inputResponses` is consumed by the bridge's elicitation hook, not the tool.
 */
function stripModernFields(
  params: Record<string, unknown>,
): Record<string, unknown> {
  const out = { ...params };
  delete out.inputResponses;
  delete out.requestState;
  const meta = out._meta;
  if (meta && typeof meta === "object") {
    const rest = { ...(meta as Record<string, unknown>) };
    delete rest[META_PROTOCOL_VERSION];
    delete rest[META_CLIENT_CAPABILITIES];
    delete rest[META_CLIENT_INFO];
    delete rest[META_LOG_LEVEL];
    if (Object.keys(rest).length === 0) delete out._meta;
    else out._meta = rest;
  }
  return out;
}

// ── Tasks execution ──────────────────────────────────────────────

function createTaskRecord(toolName: string): TaskRecord {
  const now = new Date().toISOString();
  const record: TaskRecord = {
    taskId: `task_${randomUUID().slice(0, 13)}`,
    status: "working",
    statusMessage: `running ${toolName}`,
    createdAt: now,
    lastUpdatedAt: now,
    expiresAt: Date.now() + TASK_TTL_MS,
    toolName,
    inputRequests: {},
    pendingInputs: new Map(),
    cancelRequested: false,
  };
  tasks.set(record.taskId, record);
  return record;
}

/**
 * Drive the bridged call to completion in the background. Unlike the MRTR
 * path, the bridge stays alive across `input_required`: an elicitation parks
 * the task and *waits* for `tasks/update`, so the tool resumes exactly where
 * it stopped instead of being re-run.
 */
function runTaskInBackground(
  record: TaskRecord,
  bridge: Bridge,
  method: string,
  forwarded: Record<string, unknown>,
  presetResponses: Record<string, Record<string, unknown>>,
): void {
  record.dispose = () => {
    void bridge.close();
  };

  // Rewire the elicitation path for task semantics. The hook installed at
  // openBridge time closes over these via the shared maps on the record.
  taskInputHooks.set(bridge, { record, presetResponses, seq: 0 });

  void bridge
    .rpc(method, forwarded)
    .then((reply) => {
      if (record.status === "cancelled") return;
      if (reply.error) {
        record.error = reply.error;
        touchTask(record, "failed", reply.error.message);
      } else {
        const result = { ...(reply.result ?? {}) } as Record<string, unknown>;
        result.resultType = "complete";
        record.result = result;
        touchTask(record, "completed", `${record.toolName} finished`);
      }
    })
    .catch((err) => {
      record.error = {
        code: ERR_INTERNAL,
        message: err instanceof Error ? err.message : String(err),
      };
      touchTask(record, "failed");
    })
    .finally(() => {
      taskInputHooks.delete(bridge);
      void bridge.close();
      record.dispose = undefined;
    });
}

/** Per-bridge task context so the elicitation hook can find its record. */
const taskInputHooks = new WeakMap<
  Bridge,
  {
    record: TaskRecord;
    presetResponses: Record<string, Record<string, unknown>>;
    seq: number;
  }
>();

/**
 * Elicitation raised inside a running task: park the task as `input_required`
 * and hold the tool until `tasks/update` supplies the answer (or the task is
 * cancelled). Exposed for the bridge hook installed in `handleModernRequest`.
 */
export function taskElicitation(
  bridge: object,
  reqParams: Record<string, unknown>,
): Promise<Record<string, unknown>> | undefined {
  const ctx = taskInputHooks.get(bridge as Bridge);
  if (!ctx) return undefined;
  const { record, presetResponses } = ctx;
  const key = `input_${++ctx.seq}`;
  const preset = presetResponses[key];
  if (preset) return Promise.resolve(preset);
  if (record.cancelRequested) return Promise.resolve({ action: "cancel" });
  const { elicitationId: _drop, ...cleanParams } = reqParams;
  record.inputRequests[key] = {
    method: "elicitation/create",
    params: cleanParams,
  };
  return new Promise((resolve) => {
    record.pendingInputs.set(key, resolve);
    touchTask(record, "input_required", "waiting for client input");
  });
}

function handleTaskMethod(
  id: string | number,
  method: string,
  params: Record<string, unknown>,
): ModernResponse {
  pruneTasks();
  const taskId = typeof params.taskId === "string" ? params.taskId : "";
  const record = tasks.get(taskId);
  if (!record) {
    return {
      status: 200,
      body: errorResponse(
        id,
        ERR_INVALID_PARAMS,
        `Unknown taskId '${taskId}' — expired, cancelled long ago, or from a previous server instance`,
      ),
    };
  }

  if (method === "tasks/get") {
    const result = { resultType: "complete", ...detailedTask(record) };
    return { status: 200, body: { jsonrpc: "2.0", id, result } };
  }

  if (method === "tasks/update") {
    const responses =
      params.inputResponses && typeof params.inputResponses === "object"
        ? (params.inputResponses as Record<string, Record<string, unknown>>)
        : {};
    for (const [key, response] of Object.entries(responses)) {
      const resolve = record.pendingInputs.get(key);
      if (!resolve) continue; // unknown or already-satisfied key: ignore per spec
      record.pendingInputs.delete(key);
      delete record.inputRequests[key];
      resolve(response);
    }
    if (
      record.status === "input_required" &&
      Object.keys(record.inputRequests).length === 0
    ) {
      touchTask(record, "working", `running ${record.toolName}`);
    }
    return {
      status: 200,
      body: { jsonrpc: "2.0", id, result: { resultType: "complete" } },
    };
  }

  // tasks/cancel — cooperative: unblock any parked elicitation with a cancel,
  // flag the record, and acknowledge. The background rpc may still land a
  // result; the terminal `cancelled` status wins (checked before overwrite).
  record.cancelRequested = true;
  if (record.status === "working" || record.status === "input_required") {
    for (const [key, resolve] of record.pendingInputs) {
      record.pendingInputs.delete(key);
      delete record.inputRequests[key];
      resolve({ action: "cancel" });
    }
    touchTask(record, "cancelled", "cancelled by client");
  }
  return {
    status: 200,
    body: { jsonrpc: "2.0", id, result: { resultType: "complete" } },
  };
}

// ── subscriptions/listen ─────────────────────────────────────────

export interface ListenSink {
  /** Write one JSON-RPC message to the stream. */
  send(message: unknown): void;
}

export interface ListenHandle {
  /** Detach listeners. Call when the stream closes for any reason. */
  close(): void;
}

/**
 * Serve a `subscriptions/listen` request: acknowledge the honored filter and
 * feed opted-in notifications to `sink` until `close()` is called. The caller
 * owns the transport (SSE stream on HTTP, the shared channel on stdio) and its
 * lifecycle; this function only routes events.
 *
 * Honored types: `toolsListChanged` (pack flips re-shape the surface) and the
 * Tasks extension's `taskIds`. `promptsListChanged` / `resourcesListChanged` /
 * `resourceSubscriptions` are omitted from the acknowledgment — this server's
 * prompt and resource surfaces are static, so those events can never fire, and
 * the spec says unhonored types are dropped from the ack rather than faked.
 */
export function handleModernListen(
  msg: unknown,
  sink: ListenSink,
): ListenHandle {
  const req = msg as JsonRpcRequest;
  const subscriptionId = req.id ?? null;
  const filter =
    req.params?.notifications && typeof req.params.notifications === "object"
      ? (req.params.notifications as Record<string, unknown>)
      : {};

  const honored: Record<string, unknown> = {};
  const listeners: Array<() => void> = [];

  if (filter.toolsListChanged === true) {
    honored.toolsListChanged = true;
    const onChange = () =>
      sink.send({
        jsonrpc: "2.0",
        method: "notifications/tools/list_changed",
        params: { _meta: { [META_SUBSCRIPTION_ID]: subscriptionId } },
      });
    bus.on(EVT_TOOLS_CHANGED, onChange);
    listeners.push(() => bus.off(EVT_TOOLS_CHANGED, onChange));
  }

  const taskIds = Array.isArray(filter.taskIds)
    ? (filter.taskIds as string[]).filter((t) => typeof t === "string")
    : [];
  if (taskIds.length > 0) {
    honored.taskIds = taskIds;
    const wanted = new Set(taskIds);
    const onTask = (task: Record<string, unknown>) => {
      if (!wanted.has(task.taskId as string)) return;
      sink.send({
        jsonrpc: "2.0",
        method: "notifications/tasks",
        params: { _meta: { [META_SUBSCRIPTION_ID]: subscriptionId }, ...task },
      });
    };
    bus.on(EVT_TASK, onTask);
    listeners.push(() => bus.off(EVT_TASK, onTask));
  }

  // Acknowledgment MUST precede every notification on this subscription.
  sink.send({
    jsonrpc: "2.0",
    method: "notifications/subscriptions/acknowledged",
    params: {
      _meta: { [META_SUBSCRIPTION_ID]: subscriptionId },
      notifications: honored,
    },
  });

  return {
    close: () => {
      for (const off of listeners) off();
      listeners.length = 0;
    },
  };
}

/**
 * The graceful-closure response for a listen stream the *server* is ending
 * (shutdown). Sent as the JSON-RPC response to the original request, then the
 * stream closes.
 */
export function listenClosureResponse(msg: unknown): unknown {
  const id = (msg as JsonRpcRequest).id ?? null;
  return {
    jsonrpc: "2.0",
    id,
    result: {
      resultType: "complete",
      _meta: { [META_SUBSCRIPTION_ID]: id },
    },
  };
}

/** Exposed for tests: clear all task state. */
export function resetTasksForTest(): void {
  for (const t of tasks.values()) t.dispose?.();
  tasks.clear();
}
