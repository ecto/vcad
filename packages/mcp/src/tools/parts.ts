/**
 * MCP tool handlers for the stdlib parts library — session-aware.
 *
 * `search_parts` is read-only against the manifest, no session needed.
 * `place_part` mutates the document attached to a session id (from
 * `open_document`); the chat surface in `@vcad/core` mutates a Zustand
 * docstore for the same effect.
 */

import type { Engine, PartManifestEntry } from "@vcad/engine";
import {
  loadPartsManifest,
  searchParts as searchPartsEngine,
  defaultParamsFor,
} from "@vcad/engine";
import type { Node } from "@vcad/ir";
import { getSession } from "./session.js";

export const searchPartsSchema = {
  type: "object",
  properties: {
    query: {
      type: "string",
      description:
        "Free-text search query (part name, McMaster number, ISO/DIN reference, synonym, or category).",
    },
    category: {
      type: "string",
      description: "Optional category filter ('Fasteners', 'Bearings', …).",
    },
    limit: {
      type: "integer",
      description: "Maximum number of results (default 10).",
    },
  },
} as const;

export const placePartSchema = {
  type: "object",
  properties: {
    document_id: {
      type: "string",
      description:
        "Session id from open_document. The placed part is appended to this session's document.",
    },
    path: {
      type: "string",
      description:
        "Part source path from `search_parts.id`, e.g. 'std:fastener.bolt.socket-head'.",
    },
    params: {
      type: "object",
      description:
        "Parameter name → value map. Missing params take the part's declared defaults.",
      additionalProperties: true,
    },
    position: {
      type: "object",
      description: "Optional world-space position {x, y, z} to offset the part.",
      properties: {
        x: { type: "number" },
        y: { type: "number" },
        z: { type: "number" },
      },
    },
    name: {
      type: "string",
      description: "Optional display name for the feature tree.",
    },
  },
  required: ["document_id", "path"],
} as const;

function kernelOf(engine: Engine): Parameters<typeof loadPartsManifest>[0] | null {
  // Engine hides `kernel` behind a private field. We only need the two
  // function pointers; fall back to the Solid class as a probe.
  const guess = engine as unknown as {
    kernel?: Parameters<typeof loadPartsManifest>[0];
    _kernel?: Parameters<typeof loadPartsManifest>[0];
  };
  return guess.kernel ?? guess._kernel ?? null;
}

function manifestFor(engine: Engine): PartManifestEntry[] {
  const kernel = kernelOf(engine);
  if (!kernel) return [];
  return loadPartsManifest(kernel);
}

/** `search_parts` MCP handler. */
export function searchPartsTool(
  args: Record<string, unknown>,
  engine: Engine,
): { content: Array<{ type: string; text: string }> } {
  const manifest = manifestFor(engine);
  const query = typeof args.query === "string" ? args.query : "";
  const category = typeof args.category === "string" ? args.category : undefined;
  const limit = typeof args.limit === "number" ? args.limit : 10;
  const hits = searchPartsEngine(manifest, { query, category, limit });
  const results = hits.map((p) => ({
    id: `std:${p.id}`,
    name: p.name,
    category: p.category,
    description: p.description,
    version: p.version,
    params: p.params,
    xrefs: p.xrefs,
    synonyms: p.synonyms,
  }));
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({ results, count: results.length }, null, 2),
      },
    ],
  };
}

/** `place_part` MCP handler — inserts a PartInstance node into a session
 *  document and returns the new part_id. */
export function placePartTool(
  args: Record<string, unknown>,
  engine: Engine,
): { content: Array<{ type: string; text: string }> } {
  const documentId = String(args.document_id ?? "");
  const doc = getSession(documentId);

  const path = typeof args.path === "string" ? args.path : null;
  if (!path) throw new Error("place_part: missing `path`");

  const manifest = manifestFor(engine);
  const idLookup = path.startsWith("std:") ? path.slice(4) : path;
  const entry = manifest.find((p) => p.id === idLookup);
  if (!entry) throw new Error(`place_part: unknown part "${path}"`);

  const userParams =
    args.params && typeof args.params === "object"
      ? (args.params as Record<string, unknown>)
      : {};
  const params: Record<string, unknown> = {
    ...defaultParamsFor(entry),
    ...userParams,
  };

  const existingIds = Object.keys(doc.nodes).map(Number);
  const partNodeId = (existingIds.length ? Math.max(...existingIds) : 0) + 1;

  const partNode: Node = {
    id: partNodeId,
    name: typeof args.name === "string" ? args.name : entry.name,
    op: {
      type: "PartInstance",
      path,
      version: entry.version,
      params,
    },
  };
  doc.nodes[String(partNodeId)] = partNode;

  let rootId: number = partNodeId;
  const position = args.position as { x?: number; y?: number; z?: number } | undefined;
  if (
    position &&
    (Number.isFinite(position.x) ||
      Number.isFinite(position.y) ||
      Number.isFinite(position.z))
  ) {
    const translateId = partNodeId + 1;
    doc.nodes[String(translateId)] = {
      id: translateId,
      name: null,
      op: {
        type: "Translate",
        child: partNodeId,
        offset: {
          x: typeof position.x === "number" ? position.x : 0,
          y: typeof position.y === "number" ? position.y : 0,
          z: typeof position.z === "number" ? position.z : 0,
        },
      },
    };
    rootId = translateId;
  }

  doc.roots.push({
    root: rootId,
    material: "default",
  });

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            document_id: documentId,
            part_id: String(rootId),
            placed: {
              path,
              version: entry.version,
              params,
              node_id: partNodeId,
            },
          },
          null,
          2,
        ),
      },
    ],
  };
}
