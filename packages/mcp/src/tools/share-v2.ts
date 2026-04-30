/**
 * share (v2) — handle → vcad.io URL.
 *
 * Phase-1 implementation: encodes the doc into a `?doc=` URL param the
 * way v1 did. The Phase-9 short-link variant (`vcad.io/d/<short_id>`)
 * lands once the `mcp_short_links` Supabase table is wired up — until
 * then, this is functionally equivalent to v1 but speaks the v2
 * envelope and accepts handles.
 */

import { gzipSync } from "node:zlib";
import { toVCode } from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { resolveRef } from "../handles.js";
import type { DocRef } from "../types.js";

export const shareV2Schema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle or inline IR." },
    name: { type: "string" as const, description: "Optional document name." },
    pin_version: {
      type: "boolean" as const,
      description: "When true (default), URL freezes the handle's current version.",
    },
    expires_at: { type: "string" as const },
    access: { type: "string" as const, enum: ["public", "view-link", "private"] },
  },
  required: ["doc"],
};

interface ShareV2Input {
  doc: DocRef;
  name?: string;
  pin_version?: boolean;
  expires_at?: string;
  access?: "public" | "view-link" | "private";
}

function validateAppUrl(raw: string): string {
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") return "https://vcad.io";
    const host = url.hostname;
    const ok =
      host === "localhost" ||
      host === "127.0.0.1" ||
      host === "vcad.io" ||
      host.endsWith(".vcad.io");
    return ok ? url.origin : "https://vcad.io";
  } catch {
    return "https://vcad.io";
  }
}

function base64url(b: Buffer): string {
  return b.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function shareV2(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as ShareV2Input;
  if (args.doc === undefined) return fail("invalid_input", "Missing `doc`.");

  const { doc, handle } = resolveRef(args.doc);
  const vcode = toVCode(doc);
  const compressed = gzipSync(Buffer.from(vcode, "utf-8"), { level: 9 });
  const encoded = base64url(compressed);

  const baseUrl = validateAppUrl(process.env.VCAD_APP_URL || "https://vcad.io");
  const params = new URLSearchParams();
  params.set("doc", encoded);
  if (args.name) params.set("name", args.name);

  const url = `${baseUrl}/#/new?${params.toString()}`;
  const warnings = [];
  if (url.length > 2000) {
    warnings.push({
      code: "url_too_long",
      message: `URL is ${url.length} chars (browsers may truncate >2000). Use export instead for big docs.`,
    });
  }

  return ok({
    result: {
      url,
      vcode_bytes: vcode.length,
      encoded_bytes: encoded.length,
      pin_version: args.pin_version ?? true,
      access: args.access ?? "view-link",
    },
    handle,
    doc,
    engine,
    startedAt,
    warnings,
    skipPreview: true,
  });
}
