/**
 * parts (v2) — search + place merged behind one tool.
 *
 * `mode: "search"` returns matches.
 * `mode: "place"` inserts a stdlib part into a doc handle and returns
 * the updated handle.
 */

import type { Engine, PartManifestEntry } from "@vcad/engine";
import {
  loadPartsManifest,
  searchParts as searchPartsEngine,
  defaultParamsFor,
} from "@vcad/engine";
import { createDocument, type Node } from "@vcad/ir";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { commitDoc, parseHandle, resolveRef } from "../handles.js";
import type { DocHandle, DocRef, NamedPos } from "../types.js";
import { resolvePosition } from "../refs.js";

export const partsV2Schema = {
  type: "object" as const,
  properties: {
    mode: { type: "string" as const, enum: ["search", "place"] },
    // search
    query: { type: "string" as const },
    categories: { type: "array" as const, items: { type: "string" as const } },
    limit: { type: "integer" as const },
    // place
    doc: { description: "Doc handle (place mode); optional, omit for fresh." },
    id: { type: "string" as const, description: "Part id from search.id (e.g. 'std:fastener.bolt.socket-head')." },
    at: { description: "Position (Vec3 or named anchor)." },
    params: { type: "object" as const },
    name: { type: "string" as const },
  },
  required: ["mode"],
};

interface PartsSearchInput {
  mode: "search";
  query?: string;
  categories?: string[];
  limit?: number;
}

interface PartsPlaceInput {
  mode: "place";
  doc?: DocRef;
  id: string;
  at?: { x: number; y: number; z: number } | NamedPos;
  params?: Record<string, unknown>;
  name?: string;
}

function kernelOf(engine: Engine) {
  const guess = engine as unknown as { kernel?: unknown; _kernel?: unknown };
  return (guess.kernel ?? guess._kernel) as Parameters<typeof loadPartsManifest>[0] | undefined;
}

function manifestFor(engine: Engine): PartManifestEntry[] {
  const kernel = kernelOf(engine);
  return kernel ? loadPartsManifest(kernel) : [];
}

export function partsTool(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as PartsSearchInput | PartsPlaceInput;
  if (!args.mode) return fail("invalid_input", "Missing `mode`.");

  if (args.mode === "search") {
    const a = args as PartsSearchInput;
    const manifest = manifestFor(engine);
    const category = a.categories?.[0];
    const hits = searchPartsEngine(manifest, {
      query: a.query ?? "",
      category,
      limit: a.limit ?? 10,
    });
    const matches = hits.map((p) => ({
      id: `std:${p.id}`,
      name: p.name,
      category: p.category,
      description: p.description,
      version: p.version,
      params: p.params,
      xrefs: p.xrefs,
      synonyms: p.synonyms,
    }));
    return ok({
      result: { matches, count: matches.length },
      handle: ("vcad:doc:00000000-0000-0000-0000-000000000000" as DocHandle),
      engine,
      startedAt,
      skipPreview: true,
    });
  }

  // place
  const a = args as PartsPlaceInput;
  if (!a.id) return fail("invalid_input", "place mode requires `id`.");

  const baseHandle = typeof a.doc === "string" ? a.doc : undefined;
  const { doc: source } = a.doc ? resolveRef(a.doc) : { doc: createDocument() };
  const doc = JSON.parse(JSON.stringify(source));

  const manifest = manifestFor(engine);
  const idLookup = a.id.startsWith("std:") ? a.id.slice(4) : a.id;
  const entry = manifest.find((p) => p.id === idLookup);
  if (!entry) return fail("unknown_part", `Unknown part: "${a.id}"`);

  const params: Record<string, unknown> = {
    ...defaultParamsFor(entry),
    ...(a.params ?? {}),
  };

  const existingIds = Object.keys(doc.nodes).map(Number);
  const partNodeId = (existingIds.length ? Math.max(...existingIds) : 0) + 1;
  const partNode: Node = {
    id: partNodeId,
    name: a.name ?? entry.name,
    op: { type: "PartInstance", path: a.id, version: entry.version, params },
  };
  doc.nodes[String(partNodeId)] = partNode;

  let rootId: number = partNodeId;
  if (a.at !== undefined) {
    const offset = resolvePosition(a.at as { x: number; y: number; z: number } | NamedPos);
    if (offset.x !== 0 || offset.y !== 0 || offset.z !== 0) {
      const translateId = partNodeId + 1;
      doc.nodes[String(translateId)] = {
        id: translateId,
        name: null,
        op: { type: "Translate", child: partNodeId, offset },
      };
      rootId = translateId;
    }
  }
  doc.roots.push({ root: rootId, material: "default" });

  const handle = commitDoc(doc, baseHandle as DocHandle | undefined);
  void parseHandle(handle); // sanity-check
  return ok({
    result: { added_node: rootId, placed: { id: a.id, version: entry.version, params } },
    handle,
    doc,
    engine,
    startedAt,
  });
}
