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
import {
  fetchSharedDocument,
  fetchPublicDocument,
  lookupShareRedirect,
} from "@vcad/auth";
import { decodeViewerState, type ViewerState } from "@/lib/viewer-state";

/**
 * Base64url decode (URL-safe base64 without padding). Throws a
 * descriptive Error when the input is not a valid base64 string so that
 * the caller can render a user-friendly message instead of letting an
 * unhandled exception crash the page.
 */
function base64urlDecode(str: string): Uint8Array {
  // Restore standard base64
  let base64 = str.replace(/-/g, "+").replace(/_/g, "/");
  // Add padding
  while (base64.length % 4) {
    base64 += "=";
  }
  let binary: string;
  try {
    binary = atob(base64);
  } catch {
    throw new Error("Invalid base64url in shared document URL");
  }
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
  /** Present when the doc was loaded from a share/public link (read-only). */
  readOnlyShareToken?: string;
  /** Present when the share URL carried a ?at=<encoded> viewer-state hint. */
  viewerStateHint?: ViewerState;
  /** When viewing a public doc via /@user/slug, the owner info. */
  ownerUsername?: string;
}

/** Legal page slugs served from the SPA. */
export type LegalSlug = "privacy" | "terms" | "security";

/** Route type detected from the current pathname. */
type UrlRoute =
  | { kind: "share-token"; token: string }
  | { kind: "public-doc"; username: string; slug: string }
  | { kind: "profile"; username: string }
  | { kind: "local-doc"; id: string }
  | { kind: "legal"; slug: LegalSlug }
  | null;

/** Parse the current pathname into a route. */
function parseRoute(): UrlRoute {
  if (typeof window === "undefined") return null;
  const path = window.location.pathname;

  // /privacy, /terms, /security
  const legalMatch = path.match(/^\/(privacy|terms|security)\/?$/);
  if (legalMatch?.[1]) return { kind: "legal", slug: legalMatch[1] as LegalSlug };

  // /view/<uuid-token>
  const shareMatch = path.match(/^\/view\/([0-9a-fA-F-]{8,64})\/?$/);
  if (shareMatch?.[1]) return { kind: "share-token", token: shareMatch[1] };

  // /@<username>/<slug>
  const docMatch = path.match(/^\/@([a-z0-9][a-z0-9-]*[a-z0-9])\/([a-z0-9][a-z0-9-]*)\/?$/);
  if (docMatch?.[1] && docMatch[2]) return { kind: "public-doc", username: docMatch[1], slug: docMatch[2] };

  // /@<username> (profile page — no slug)
  const profileMatch = path.match(/^\/@([a-z0-9][a-z0-9-]*[a-z0-9])\/?$/);
  if (profileMatch?.[1]) return { kind: "profile", username: profileMatch[1] };

  // /d/<id> — local IndexedDB document identity. Accepts both legacy UUIDs
  // (hex + dashes) and short base62 nanoids minted by newDocId().
  const localMatch = path.match(/^\/d\/([A-Za-z0-9_-]{8,64})\/?$/);
  if (localMatch?.[1]) return { kind: "local-doc", id: localMatch[1] };

  return null;
}

/**
 * Returns the local document ID from the current URL, if the user landed on
 * a `/d/<id>` path. Bootstrap checks this before falling back to
 * "most recent document" so refresh always reopens the exact doc.
 */
export function getLocalDocRouteId(): string | null {
  const route = parseRoute();
  return route?.kind === "local-doc" ? route.id : null;
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
  const route = parseRoute();

  // 1. /@username/slug — Phase 1 canonical public doc URLs
  if (route?.kind === "public-doc") {
    try {
      const pub = await fetchPublicDocument(route.username, route.slug);
      if (!pub) {
        console.warn("[url-document] public doc not found:", route.username, route.slug);
        return null;
      }
      const file =
        typeof pub.content === "string"
          ? parseVcadFile(pub.content)
          : parseVcadFile(JSON.stringify(pub.content));
      const viewerStateHint = parseViewerStateHint() ?? undefined;
      return {
        file,
        name: pub.name,
        readOnlyShareToken: `@${route.username}/${route.slug}`,
        viewerStateHint,
        ownerUsername: pub.owner_username,
      };
    } catch (err) {
      console.error("[url-document] failed to load public doc:", err);
      return null;
    }
  }

  // 2. /view/<token> — Phase 0 share links (check for redirect first)
  if (route?.kind === "share-token") {
    try {
      // Check if this token has been upgraded to a canonical /@user/slug URL
      const redirect = await lookupShareRedirect(route.token);
      if (redirect) {
        window.location.replace(`/@${redirect.username}/${redirect.slug}`);
        return null; // navigation will reload the page
      }

      const shared = await fetchSharedDocument(route.token);
      if (!shared) {
        console.warn("[url-document] share token invalid or revoked");
        return null;
      }
      const file =
        typeof shared.content === "string"
          ? parseVcadFile(shared.content)
          : parseVcadFile(JSON.stringify(shared.content));
      const viewerStateHint = parseViewerStateHint() ?? undefined;
      return {
        file,
        name: shared.name,
        readOnlyShareToken: route.token,
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
  return parseUrlParams() !== null || parseRoute() !== null;
}

/**
 * Check if the current URL is a profile page (/@username with no slug).
 * The app renders a ProfilePage instead of the normal editor in this case.
 */
export function getProfileRouteUsername(): string | null {
  const route = parseRoute();
  return route?.kind === "profile" ? route.username : null;
}

/**
 * Check if the current URL is a legal page (/privacy, /terms, /security).
 * The app renders a LegalPage instead of the normal editor in this case.
 */
export function getLegalRouteSlug(): LegalSlug | null {
  const route = parseRoute();
  return route?.kind === "legal" ? route.slug : null;
}
