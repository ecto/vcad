/**
 * Document session management for MCP. Mirrors the gym tools' pattern
 * (`simulations: Map<string, PhysicsEnv>` in tools/gym.ts) — each session
 * holds a `Document` that subsequent CRUD calls mutate in place.
 *
 * The chat surface in the web app does the same thing implicitly via the
 * Zustand `useDocumentStore`. MCP needs an explicit handle because each
 * call is stateless across the wire.
 */

import { createDocument } from "@vcad/ir";
import type { Document } from "@vcad/ir";
import { writeFileSync, readFileSync } from "node:fs";
import { resolveWithinRoot } from "./safe-path.js";

/** Session id → Document. Lives for the lifetime of the MCP server
 *  process, like `simulations` and `batchGroups` in tools/gym.ts. */
export const documents = new Map<string, Document>();

let nextId = 1;

function nextSessionId(): string {
  return `doc_${Date.now()}_${nextId++}`;
}

/** Register a freshly-built document as a session and return its id.
 *  Lets other tools (e.g. `sheet_metal_create`) hand back a
 *  `document_id` that `inspect_cad` / `export_cad` / `open_in_browser`
 *  can then operate on, without duplicating the id scheme. */
export function registerSession(doc: Document): string {
  const id = nextSessionId();
  documents.set(id, doc);
  return id;
}

/** Get a session document by id, or throw a helpful error. */
export function getSession(documentId: string): Document {
  const doc = documents.get(documentId);
  if (!doc) {
    throw new Error(
      `Unknown document_id "${documentId}". Open one with open_document first, or list active sessions with the documents map.`,
    );
  }
  return doc;
}

// ─── open_document ────────────────────────────────────────────────────────

export const openDocumentSchema = {
  type: "object" as const,
  properties: {
    initial: {
      type: "object" as const,
      description:
        "Optional initial Document IR. If omitted, an empty document is created. Pass an existing IR (e.g. from import_step) to begin editing it.",
    },
  },
};

export function openDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const initial = args.initial as Document | undefined;
  const doc: Document = initial
    ? // Defensive copy — callers shouldn't be able to mutate the session
      // doc by retaining the reference they passed in.
      JSON.parse(JSON.stringify(initial))
    : createDocument();
  const id = nextSessionId();
  documents.set(id, doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ document_id: id, parts: doc.roots.length }),
      },
    ],
  };
}

// ─── get_document ─────────────────────────────────────────────────────────

export const getDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
  },
  required: ["document_id"],
};

export function getDocumentTool(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const id = String(args.document_id ?? "");
  const doc = getSession(id);
  return {
    content: [{ type: "text", text: JSON.stringify(doc) }],
  };
}

// ─── close_document ───────────────────────────────────────────────────────

export const closeDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
  },
  required: ["document_id"],
};

export function closeDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const id = String(args.document_id ?? "");
  const existed = documents.delete(id);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ closed: existed, document_id: id }),
      },
    ],
  };
}

// ─── Durable persistence (save_document / load_document) ──────────────────────
//
// The session Map above is in-process only — a server restart or cold start
// loses every board, and there is no way to reopen one by id. These two tools
// add a file-backed persistence layer: `save_document` serializes a live
// session to `<name>.vcad` under the state root, and `load_document` reads it
// back into a fresh session.
//
// The state root is VCAD_MCP_STATE_DIR (or process.cwd()), and `resolveWithinRoot`
// both sanitizes `name` and confines reads/writes to that root, so a caller can
// never escape it with `../` or an absolute path.
//
// This is the local/stdio persistence layer. The natural extension for the
// hosted/multi-tenant deployment is durable storage in the Supabase `documents`
// table keyed by the OAuth user, rather than the local filesystem.

/** State root for saved `.vcad` files. VCAD_MCP_STATE_DIR or cwd. */
function stateRoot(): string {
  return process.env.VCAD_MCP_STATE_DIR ?? process.cwd();
}

// ─── save_document ────────────────────────────────────────────────────────

export const saveDocumentSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
    name: {
      type: "string" as const,
      description:
        "Filename slug (no extension) to save under, relative to the server state directory " +
        "(VCAD_MCP_STATE_DIR if set, otherwise the working directory). Written as <name>.vcad.",
    },
  },
  required: ["document_id", "name"],
};

export function saveDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
} {
  const id = String(args.document_id ?? "");
  const name = String(args.name ?? "");
  const doc = getSession(id);
  // resolveWithinRoot sanitizes `name` (rejects absolute/.. /NUL/escape) and
  // confines the write to the state root.
  const path = resolveWithinRoot(`${name}.vcad`, stateRoot());
  writeFileSync(path, JSON.stringify(doc));
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ saved: true, name, path }),
      },
    ],
  };
}

// ─── load_document ────────────────────────────────────────────────────────

export const loadDocumentSchema = {
  type: "object" as const,
  properties: {
    name: {
      type: "string" as const,
      description:
        "Filename slug (no extension) to load, relative to the server state directory " +
        "(VCAD_MCP_STATE_DIR if set, otherwise the working directory). Reads <name>.vcad.",
    },
  },
  required: ["name"],
};

export function loadDocument(args: Record<string, unknown>): {
  content: Array<{ type: "text"; text: string }>;
  isError?: boolean;
} {
  const name = String(args.name ?? "");
  const root = stateRoot();
  const path = resolveWithinRoot(`${name}.vcad`, root);
  let raw: string;
  try {
    raw = readFileSync(path, "utf8");
  } catch {
    return {
      isError: true,
      content: [
        {
          type: "text",
          text: `No saved document named "${name}" under ${root}`,
        },
      ],
    };
  }
  const doc = JSON.parse(raw) as Document;
  const id = registerSession(doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          document_id: id,
          name,
          parts: doc.roots?.length ?? 0,
        }),
      },
    ],
  };
}
