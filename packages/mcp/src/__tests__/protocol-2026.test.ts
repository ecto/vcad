import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import { createServer } from "../server.js";
import {
  PROTOCOL_2026,
  SUPPORTED_PROTOCOL_VERSIONS,
  META_PROTOCOL_VERSION,
  META_CLIENT_INFO,
  META_CLIENT_CAPABILITIES,
  META_SERVER_INFO,
  ERR_HEADER_MISMATCH,
  ERR_UNSUPPORTED_PROTOCOL_VERSION,
  isModernMessage,
  handleModernRequest,
} from "../protocol-2026.js";

/**
 * MCP 2026-07-28 conformance for the dual-era front end.
 *
 * These drive `handleModernRequest` exactly as the HTTP entry does — raw
 * JSON-RPC in, `{status, body}` out — so they cover the wire contract a real
 * modern client sees: no `initialize`, no session header, `server/discover`
 * for negotiation, `resultType` on every result, and header/body agreement.
 */

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
}, 60_000);

const makeServer = () => createServer(engine, { user: null, assumeUiClient: true });

const META = {
  [META_PROTOCOL_VERSION]: PROTOCOL_2026,
  [META_CLIENT_INFO]: { name: "conformance-test", version: "1.0.0" },
  [META_CLIENT_CAPABILITIES]: {},
};

/** Build the headers a conforming client mirrors onto the POST. */
function headersFor(method: string, name?: string): Record<string, string> {
  const h: Record<string, string> = {
    "mcp-protocol-version": PROTOCOL_2026,
    "mcp-method": method,
  };
  if (name !== undefined) h["mcp-name"] = name;
  return h;
}

function request(
  id: number | string,
  method: string,
  params: Record<string, unknown> = {},
): Record<string, unknown> {
  return { jsonrpc: "2.0", id, method, params: { ...params, _meta: META } };
}

interface JsonRpcResult {
  result?: Record<string, unknown>;
  error?: { code: number; message: string; data?: unknown };
}

async function call(
  msg: Record<string, unknown>,
  headers?: Record<string, string>,
): Promise<{ status: number; body: JsonRpcResult }> {
  const res = await handleModernRequest(msg, {
    createServer: makeServer,
    headers,
  });
  return { status: res.status, body: res.body as JsonRpcResult };
}

describe("era detection", () => {
  it("routes a body that declares a protocol version to the modern handler", () => {
    expect(isModernMessage(request(1, "tools/list"))).toBe(true);
  });

  it("routes server/discover to the modern handler even without _meta", () => {
    expect(
      isModernMessage({ jsonrpc: "2.0", id: 1, method: "server/discover" }),
    ).toBe(true);
  });

  it("leaves a legacy initialize handshake on the SDK path", () => {
    // A 2025-11-25 client sends MCP-Protocol-Version too — the header alone
    // must never be read as a modern signal, or every legacy client breaks.
    expect(
      isModernMessage(
        {
          jsonrpc: "2.0",
          id: 1,
          method: "initialize",
          params: { protocolVersion: "2025-11-25", capabilities: {} },
        },
        { "mcp-protocol-version": "2025-11-25" },
      ),
    ).toBe(false);
  });
});

describe("server/discover", () => {
  it("reports supported versions, capabilities, and identity", async () => {
    const { status, body } = await call(
      request("d1", "server/discover"),
      headersFor("server/discover"),
    );
    expect(status).toBe(200);
    const result = body.result!;
    expect(result.resultType).toBe("complete");
    expect(result.supportedVersions).toEqual(SUPPORTED_PROTOCOL_VERSIONS);
    expect(SUPPORTED_PROTOCOL_VERSIONS[0]).toBe(PROTOCOL_2026);
    expect(result.capabilities).toMatchObject({ tools: { listChanged: true } });
    const meta = result._meta as Record<string, { name?: string }>;
    expect(meta[META_SERVER_INFO].name).toBe("vcad");
    expect(typeof result.instructions).toBe("string");
    // CacheableResult: a discovery response is cacheable, and private because
    // the tool surface varies with the caller's auth and enabled packs.
    expect(result.ttlMs).toBeGreaterThan(0);
    expect(result.cacheScope).toBe("private");
  });

  it("answers a probe that has not yet chosen a version", async () => {
    const { status, body } = await call(
      { jsonrpc: "2.0", id: "d2", method: "server/discover" },
      headersFor("server/discover"),
    );
    expect(status).toBe(200);
    expect(body.result!.supportedVersions).toEqual(SUPPORTED_PROTOCOL_VERSIONS);
  });
});

describe("tools", () => {
  it("lists tools without any initialize handshake", async () => {
    const { status, body } = await call(
      request(1, "tools/list"),
      headersFor("tools/list"),
    );
    expect(status).toBe(200);
    const tools = body.result!.tools as { name: string }[];
    expect(tools.length).toBeGreaterThan(10);
    expect(tools.some((t) => t.name === "open_document")).toBe(true);
    expect(body.result!.resultType).toBe("complete");
    expect(body.result!.ttlMs).toBeGreaterThan(0);
  });

  it("returns tools in a deterministic order across calls", async () => {
    // SHOULD in 2026-07-28: stable order lets clients cache the catalog and
    // keeps the LLM prompt prefix cacheable.
    const [a, b] = await Promise.all([
      call(request(1, "tools/list"), headersFor("tools/list")),
      call(request(2, "tools/list"), headersFor("tools/list")),
    ]);
    const names = (r: typeof a) =>
      (r.body.result!.tools as { name: string }[]).map((t) => t.name);
    expect(names(a)).toEqual(names(b));
  });

  it("calls a tool and carries state on a server-minted handle, not a session", async () => {
    const opened = await call(
      request(1, "tools/call", { name: "open_document", arguments: {} }),
      headersFor("tools/call", "open_document"),
    );
    expect(opened.status).toBe(200);
    expect(opened.body.result!.resultType).toBe("complete");
    const text = (opened.body.result!.content as { text: string }[])[0].text;
    const documentId = JSON.parse(text).document_id as string;
    expect(documentId).toBeTruthy();

    // A completely independent request — served by a different Server
    // instance, with no session id anywhere on the wire — resolves the same
    // document through the handle passed as an ordinary tool argument. This is
    // the state model 2026-07-28 prescribes now that protocol sessions are gone.
    const read = await call(
      request(2, "tools/call", {
        name: "read",
        arguments: { document_id: documentId },
      }),
      headersFor("tools/call", "read"),
    );
    expect(read.status).toBe(200);
    expect(read.body.result!.isError).toBeFalsy();
    expect((read.body.result!.content as { text: string }[])[0].text).not.toMatch(
      /Unknown document_id/,
    );
  });

  it("stamps serverInfo into every result's _meta", async () => {
    const { body } = await call(
      request(1, "tools/list"),
      headersFor("tools/list"),
    );
    const meta = body.result!._meta as Record<string, { name?: string }>;
    expect(meta[META_SERVER_INFO].name).toBe("vcad");
  });
});

describe("version negotiation", () => {
  it("rejects an unsupported version with the supported list", async () => {
    const msg = request(1, "tools/list");
    (msg.params as Record<string, Record<string, unknown>>)._meta = {
      ...META,
      [META_PROTOCOL_VERSION]: "1900-01-01",
    };
    const { status, body } = await call(msg, {
      ...headersFor("tools/list"),
      "mcp-protocol-version": "1900-01-01",
    });
    expect(status).toBe(400);
    expect(body.error!.code).toBe(ERR_UNSUPPORTED_PROTOCOL_VERSION);
    expect(body.error!.data).toEqual({
      supported: SUPPORTED_PROTOCOL_VERSIONS,
      requested: "1900-01-01",
    });
  });
});

describe("header validation", () => {
  it("rejects a Mcp-Method header that disagrees with the body", async () => {
    const { status, body } = await call(
      request(1, "tools/list"),
      headersFor("resources/list"),
    );
    expect(status).toBe(400);
    expect(body.error!.code).toBe(ERR_HEADER_MISMATCH);
  });

  it("rejects a missing Mcp-Name on tools/call", async () => {
    const { status, body } = await call(
      request(1, "tools/call", { name: "open_document", arguments: {} }),
      headersFor("tools/call"),
    );
    expect(status).toBe(400);
    expect(body.error!.code).toBe(ERR_HEADER_MISMATCH);
  });

  it("rejects a MCP-Protocol-Version header that disagrees with the body", async () => {
    const { status, body } = await call(request(1, "tools/list"), {
      ...headersFor("tools/list"),
      "mcp-protocol-version": "2025-11-25",
    });
    expect(status).toBe(400);
    expect(body.error!.code).toBe(ERR_HEADER_MISMATCH);
  });

  it("accepts a base64-sentinel Mcp-Name", async () => {
    const encoded = `=?base64?${Buffer.from("open_document", "utf-8").toString("base64")}?=`;
    const { status } = await call(
      request(1, "tools/call", { name: "open_document", arguments: {} }),
      { ...headersFor("tools/call"), "mcp-name": encoded },
    );
    expect(status).toBe(200);
  });

  it("skips header checks on a transport that has none (stdio)", async () => {
    const { status } = await call(request(1, "tools/list"));
    expect(status).toBe(200);
  });
});

describe("removed and unimplemented methods", () => {
  it.each(["ping", "logging/setLevel", "resources/subscribe", "tasks/list"])(
    "reports %s — removed in this revision — as method-not-found with 404",
    async (method) => {
      const { status, body } = await call(
        request(1, method),
        headersFor(method),
      );
      expect(status).toBe(404);
      expect(body.error!.code).toBe(-32601);
    },
  );

  it("names its supported versions when a legacy handshake reaches the modern path", async () => {
    // A legacy client has no fall-forward mechanism, so this error may be the
    // only diagnostic its user ever sees.
    const { status, body } = await call(
      request(1, "initialize"),
      headersFor("initialize"),
    );
    expect(status).toBe(404);
    expect(body.error!.code).toBe(-32601);
    expect(body.error!.message).toContain(PROTOCOL_2026);
    expect(body.error!.message).toContain("2025-11-25");
  });

  it("reports subscriptions/listen as unimplemented rather than silently accepting", async () => {
    const { status, body } = await call(
      request(1, "subscriptions/listen"),
      headersFor("subscriptions/listen"),
    );
    expect(status).toBe(404);
    expect(body.error!.code).toBe(-32601);
    expect(body.error!.message).toMatch(/stateless/);
  });

  it("acknowledges a notification with 202 and no body", async () => {
    const res = await handleModernRequest(
      { jsonrpc: "2.0", method: "notifications/whatever", params: { _meta: META } },
      { createServer: makeServer },
    );
    expect(res.status).toBe(202);
    expect(res.body).toBeNull();
  });

  it("rejects a JSON-RPC batch", async () => {
    const res = await handleModernRequest([request(1, "tools/list")], {
      createServer: makeServer,
    });
    expect(res.status).toBe(400);
  });
});

describe("resources", () => {
  it("renumbers resource-not-found to Invalid Params (-32602)", async () => {
    const { body } = await call(
      request(1, "resources/read", { uri: "ui://vcad/does-not-exist" }),
      headersFor("resources/read", "ui://vcad/does-not-exist"),
    );
    expect(body.error!.code).toBe(-32602);
  });
});
