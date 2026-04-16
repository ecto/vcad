/**
 * Load documents from URL parameters.
 *
 * URL format: https://vcad.io/#/new?doc=<compressed>&name=<name>
 *
 * The `doc` parameter contains gzip-compressed, base64url-encoded VCode.
 *
 * Also handles /view/<token> paths for Phase 0 public share links — fetches
 * the shared doc via the get_shared_document RPC and returns it alongside a
 * read-only flag and optional viewer-state hint (from the ?at= query param).
 */

import { parseVcadFile, type VcadFile } from "@vcad/core";
import { fetchSharedDocument } from "@vcad/auth";
import { decodeViewerState, type ViewerState } from "@/lib/viewer-state";

/**
 * Base64url decode (URL-safe base64 without padding).
 */
function base64urlDecode(str: string): Uint8Array {
  // Restore standard base64
  let base64 = str.replace(/-/g, "+").replace(/_/g, "/");
  // Add padding
  while (base64.length % 4) {
    base64 += "=";
  }
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

/**
 * Decompress gzip data using DecompressionStream (native browser API).
 */
async function decompressGzip(data: Uint8Array): Promise<string> {
  // Use Response + DecompressionStream for cleaner API
  // Cast to BlobPart to satisfy TypeScript's strict buffer type checking
  const blob = new Blob([data as unknown as BlobPart]);
  const ds = new DecompressionStream("gzip");
  const decompressedStream = blob.stream().pipeThrough(ds);
  const decompressedBlob = await new Response(decompressedStream).blob();
  return decompressedBlob.text();
}

export interface UrlDocumentParams {
  doc: string;
  name?: string;
  raw?: boolean; // If true, doc is raw VCode (not compressed)
}

/**
 * Parse URL hash parameters.
 * Expects format: #/new?doc=<compressed>&name=<name>
 * Also supports: ?ir=<url-encoded-vcode> for simple sharing
 */
export function parseUrlParams(): UrlDocumentParams | null {
  // Check for simple ?ir= parameter first (uncompressed, URL-encoded)
  const searchParams = new URLSearchParams(window.location.search);
  const rawIr = searchParams.get("ir");
  if (rawIr) {
    return {
      doc: rawIr,
      name: searchParams.get("name") ?? undefined,
      raw: true, // Flag to skip decompression
    };
  }

  // Check for hash-based compressed format
  const hash = window.location.hash;
  if (!hash.startsWith("#/new?")) {
    return null;
  }

  const queryString = hash.slice(6); // Remove "#/new?"
  const params = new URLSearchParams(queryString);

  const doc = params.get("doc");
  if (!doc) {
    return null;
  }

  return {
    doc,
    name: params.get("name") ?? undefined,
  };
}

export interface UrlDocumentResult {
  file: VcadFile;
  name: string;
  /** Present when the doc was loaded from a /view/<token> share link. */
  readOnlyShareToken?: string;
  /** Present when the share URL carried a ?at=<encoded> viewer-state hint. */
  viewerStateHint?: ViewerState;
}

/** Detect /view/<token> on the current pathname. Returns the token or null. */
function parseShareTokenFromPath(): string | null {
  if (typeof window === "undefined") return null;
  const match = window.location.pathname.match(
    /^\/view\/([0-9a-fA-F-]{8,64})\/?$/,
  );
  return match?.[1] ?? null;
}

/** Read and decode the optional ?at=<encoded> viewer state hint. */
function parseViewerStateHint(): ViewerState | null {
  if (typeof window === "undefined") return null;
  const at = new URLSearchParams(window.location.search).get("at");
  if (!at) return null;
  return decodeViewerState(at);
}

/**
 * Load a document from URL parameters.
 * Returns null if no URL document is present or loading fails.
 */
export async function loadDocumentFromUrl(): Promise<UrlDocumentResult | null> {
  // 1. /view/<token> — Phase 0 public share links
  const shareToken = parseShareTokenFromPath();
  if (shareToken) {
    try {
      const shared = await fetchSharedDocument(shareToken);
      if (!shared) {
        console.warn("[url-document] share token invalid or revoked");
        return null;
      }
      // The RPC returns content as `unknown` (jsonb). parseVcadFile validates.
      const file =
        typeof shared.content === "string"
          ? parseVcadFile(shared.content)
          : parseVcadFile(JSON.stringify(shared.content));
      const viewerStateHint = parseViewerStateHint() ?? undefined;
      // Do NOT clear the URL — we want the read-only share URL to persist
      // across reloads so the viewer stays in the shared session.
      return {
        file,
        name: shared.name,
        readOnlyShareToken: shareToken,
        viewerStateHint,
      };
    } catch (err) {
      console.error("[url-document] failed to load shared doc:", err);
      return null;
    }
  }

  // 2. Existing hash-based VCode shares + ?ir= raw shares
  const params = parseUrlParams();
  if (!params) {
    return null;
  }

  try {
    let compact: string;

    if (params.raw) {
      // Raw VCode (URL-decoded by URLSearchParams)
      compact = params.doc;
    } else {
      // Decode and decompress base64url + gzip
      const compressed = base64urlDecode(params.doc);
      compact = await decompressGzip(compressed);
    }

    // Parse VCode into VcadFile
    const file = parseVcadFile(compact);

    // Clear the URL to prevent re-loading on refresh
    window.history.replaceState(null, "", window.location.pathname);

    return {
      file,
      name: params.name ?? "Shared Document",
    };
  } catch (err) {
    console.error("Failed to load document from URL:", err);
    return null;
  }
}

/**
 * Check if the current URL has document parameters.
 */
export function hasUrlDocument(): boolean {
  return parseUrlParams() !== null || parseShareTokenFromPath() !== null;
}
