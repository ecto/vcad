/**
 * continue_document — open an MCP editing session from a vcad.io web document's
 * public share token. The server side of the web app's "Continue in Claude"
 * handoff: the browser mints a share token for the part the user is looking at,
 * hands it to the model in the seed prompt, and the model calls this tool. The
 * geometry never rides the URL — only the opaque token does.
 *
 * The web doc's canonical stored content is CRDT state (`{replica_id, ops}`),
 * which has no pre-materialized IR, so we materialize it through the kernel
 * engine's `WasmDocumentEngine.load()` — the same engine `render_view` already
 * boots. Raw-IR content (an MCP-authored or legacy doc) is passed through.
 *
 * The opened session is a NEW `mcp:`-keyed session seeded from the web doc's
 * geometry — continuation in Claude is its own session, exactly like every other
 * MCP-authored document, so it renders at vcad.io without colliding with the
 * original web row. (Streaming edits back into the *same* open web tab is the
 * separate live-mitosis path.)
 */
import { getKernelWasm } from "@vcad/engine";
import { fromVCode } from "@vcad/ir";
import type { Document } from "@vcad/ir";
import { gunzipSync } from "node:zlib";
import { resolveShareToken } from "../session-store.js";
import { registerSession } from "./session.js";

export const continueDocumentSchema = {
  type: "object" as const,
  properties: {
    token: {
      type: "string" as const,
      description:
        "The vcad.io share token from a 'Continue in Claude' handoff (a UUID). " +
        "Opens the signed-in user's current part as an editing session you can " +
        "render, measure, and continue.",
    },
    doc: {
      type: "string" as const,
      description:
        "An inline, compressed document handoff (gzip + base64url of the IR), " +
        "used when the part was handed off without a cloud account. Supply " +
        "either `token` or `doc`.",
    },
  },
};

type ToolText = {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
};

function err(text: string): ToolText {
  return { isError: true, content: [{ type: "text", text }] };
}

function looksLikeIr(c: unknown): c is Document {
  return (
    !!c &&
    typeof c === "object" &&
    "nodes" in (c as object) &&
    "roots" in (c as object)
  );
}

function looksLikeCrdt(c: unknown): boolean {
  return (
    !!c &&
    typeof c === "object" &&
    "replica_id" in (c as object) &&
    Array.isArray((c as { ops?: unknown }).ops)
  );
}

/** Decode an inline `doc` handoff (gzip + base64url of VCode or IR JSON) into
 *  raw content for {@link materialize}. Mirrors the encoding open_in_browser /
 *  the web app produce. Throws on a corrupt blob — the caller turns that into a
 *  re-share hint. */
function decodeInlineDoc(blob: string): unknown {
  const b64 = blob.replace(/-/g, "+").replace(/_/g, "/");
  const buf = Buffer.from(b64, "base64");
  const text = gunzipSync(buf).toString("utf-8").trim();
  // The web app gzips raw IR JSON; open_in_browser may emit VCode (`#…`).
  return text.startsWith("#") ? fromVCode(text) : JSON.parse(text);
}

/** Materialize raw cloud `content` into an IR Document. Returns null for shapes
 *  this build can't open (e.g. loon-only, or CRDT when the engine is absent). */
async function materialize(content: unknown): Promise<Document | null> {
  // Already IR — defensive copy so the caller can't mutate a shared reference.
  if (looksLikeIr(content)) {
    return JSON.parse(JSON.stringify(content)) as Document;
  }
  if (looksLikeCrdt(content)) {
    const wasm = (await getKernelWasm()) as unknown as {
      WasmDocumentEngine?: {
        load(bytes: Uint8Array): { get_document_json(): string };
      };
    };
    const Engine = wasm?.WasmDocumentEngine;
    if (!Engine) return null;
    // The web app stores CRDT as JSON; `crdtBytes` is its UTF-8 encoding
    // (see cloudContentToVcadFile). Mirror that so `load()` accepts it.
    const bytes = new TextEncoder().encode(JSON.stringify(content));
    const engine = Engine.load(bytes);
    return JSON.parse(engine.get_document_json()) as Document;
  }
  return null;
}

export async function continueDocument(
  args: Record<string, unknown>,
): Promise<ToolText> {
  const token = String(args.token ?? "").trim();
  const inlineDoc = String(args.doc ?? "").trim();
  if (!token && !inlineDoc) {
    return err(
      "continue_document requires a `token` (a vcad.io share token) or an " +
        "inline `doc` from the Continue in Claude handoff.",
    );
  }

  // Resolve the raw content + a display name from whichever source was given.
  let content: unknown;
  let name = "Shared part";
  if (token) {
    const resolved = await resolveShareToken(token);
    if (!resolved) {
      return err(
        `No shared document for token "${token}". Ask the user to click ` +
          `"Continue in Claude" again from vcad.io to mint a fresh handoff link.`,
      );
    }
    content = resolved.content;
    name = resolved.name;
  } else {
    try {
      content = decodeInlineDoc(inlineDoc);
    } catch (e) {
      return err(
        `Couldn't read the inline handoff: ${(e as Error).message}. ` +
          `Ask the user to click "Continue in Claude" again from vcad.io.`,
      );
    }
  }

  let doc: Document | null;
  try {
    doc = await materialize(content);
  } catch (e) {
    return err(
      `Could not open the shared document: ${(e as Error).message}. ` +
        `Ask the user to re-share from vcad.io.`,
    );
  }
  if (!doc) {
    return err(
      "The shared document is in a format this build can't open yet " +
        "(expected CRDT or IR content).",
    );
  }

  const id = registerSession(doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: id,
          parts: doc.roots?.length ?? 0,
          name,
        }),
      },
    ],
  };
}
