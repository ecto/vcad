/**
 * open_in_browser tool — generate shareable vcad.io URLs.
 */

import type { Document } from "@vcad/ir";
import { fromVCode, toVCode } from "@vcad/ir";
import { gzipSync } from "node:zlib";

interface ShareInput {
  document: string;  // JSON string or VCode
  name?: string;
}

export const openInBrowserSchema = {
  type: "object" as const,
  properties: {
    document: {
      type: "string" as const,
      description: "IR document (JSON or VCode format)",
    },
    name: {
      type: "string" as const,
      description: "Optional document name",
    },
  },
  required: ["document"],
};

/**
 * Base64url encode (URL-safe base64 without padding).
 */
function base64urlEncode(data: Buffer): string {
  return data
    .toString("base64")
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

/**
 * Parse a document from JSON or VCode format.
 */
function parseDocument(input: string): Document {
  const trimmed = input.trim();
  if (trimmed.startsWith("{")) {
    return JSON.parse(trimmed) as Document;
  }
  if (trimmed.startsWith("#")) {
    return fromVCode(trimmed);
  }
  throw new Error("Document must be JSON (starting with '{') or VCode (starting with '#')");
}

/**
 * Compress a VCode document for URL embedding.
 * Format: gzip + base64url
 */
function compressForUrl(compact: string): string {
  const compressed = gzipSync(Buffer.from(compact, "utf-8"), { level: 9 });
  return base64urlEncode(compressed);
}

export function openInBrowser(
  input: unknown,
): { content: Array<{ type: "text"; text: string }> } {
  const { document: docInput, name } = input as ShareInput;

  // Parse and convert to compact (smallest representation)
  const doc = parseDocument(docInput);
  const vcode = toVCode(doc);

  // Compress for URL
  const encoded = compressForUrl(vcode);

  // Build URL
  const baseUrl = process.env.VCAD_APP_URL || "https://vcad.io";
  const params = new URLSearchParams();
  params.set("doc", encoded);
  if (name) {
    params.set("name", name);
  }

  const url = `${baseUrl}/#/new?${params.toString()}`;

  // Check URL length (browsers typically support ~2KB in URL)
  const urlLength = url.length;
  const warning = urlLength > 2000
    ? `\n\nWarning: URL is ${urlLength} characters. Some browsers may truncate URLs over ~2000 characters. Consider exporting to a file instead for larger documents.`
    : "";

  return {
    content: [
      {
        type: "text",
        text: `Open in vcad.io:\n${url}${warning}\n\nVCode size: ${compact.length} bytes\nCompressed URL param: ${encoded.length} bytes`,
      },
    ],
  };
}
