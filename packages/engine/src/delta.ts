/**
 * Consumer side of engine command deltas (`vcad_app::delta`).
 *
 * `WasmDocumentEngine.applyDelta` / `applyToolDelta` answer a command with
 * an `EngineDeltaEnvelope` — a patch to the previous IR document plus the
 * parts added / updated / removed by stable id — instead of the whole
 * materialized document. A host keeps an {@link EngineState} (seeded from
 * the full-document envelope, e.g. `stateEnvelope()`) and folds each delta
 * in with {@link applyEngineDelta}. The Rust property test
 * (`deltas_reproduce_the_full_materialize`) guarantees the fold equals the
 * full materialize.
 */
import type {
  Document,
  DocumentDelta,
  EngineDelta,
  EngineDeltaEnvelope,
  EngineEnvelope,
  EngineError,
  EnginePartInfo,
  PartsDelta,
} from "@vcad/ir";

/** What a host renders: the materialized document and its parts. */
export interface EngineState {
  document: Document;
  parts: EnginePartInfo[];
}

/** Stable ids a delta asks the host to (re)build: added + updated parts. */
export interface DeltaChanges {
  rebuild: string[];
  removed: string[];
}

/** Thrown for a refused command; carries the engine's structured error. */
export class EngineRefusal extends Error {
  constructor(readonly error: EngineError) {
    super(error.message);
    this.name = "EngineRefusal";
  }
}

/** Seed an {@link EngineState} from a full-document envelope. */
export function stateFromEnvelope(json: string | EngineEnvelope): EngineState {
  const env: EngineEnvelope = typeof json === "string" ? JSON.parse(json) : json;
  if (!env.ok || !env.result) throw new EngineRefusal(env.error!);
  return { document: env.result.document, parts: env.result.parts };
}

/** Parse a delta envelope; throws {@link EngineRefusal} when refused. */
export function parseDeltaEnvelope(json: string | EngineDeltaEnvelope): EngineDelta {
  const env: EngineDeltaEnvelope = typeof json === "string" ? JSON.parse(json) : json;
  if (!env.ok || !env.result) throw new EngineRefusal(env.error!);
  return env.result;
}

/** Patch an IR document (as plain JSON) with a {@link DocumentDelta}; returns a new object. */
export function applyDocumentDelta(doc: Document, delta: DocumentDelta): Document {
  const out: Record<string, unknown> = { ...(doc as unknown as Record<string, unknown>) };
  for (const key of delta.removedFields) delete out[key];
  for (const [key, value] of Object.entries(delta.fields)) out[key] = value;
  for (const [key, patch] of Object.entries(delta.maps)) {
    const prev = out[key];
    const map: Record<string, unknown> =
      prev && typeof prev === "object" && !Array.isArray(prev) ? { ...(prev as Record<string, unknown>) } : {};
    for (const k of patch.remove) delete map[k];
    for (const [k, v] of Object.entries(patch.set)) map[k] = v;
    out[key] = map;
  }
  return out as unknown as Document;
}

/** Patch a part list with a {@link PartsDelta}; returns a new array. */
export function applyPartsDelta(parts: EnginePartInfo[], delta: PartsDelta): EnginePartInfo[] {
  const order = delta.order ?? parts.map((p) => p.id);
  const byId = new Map(parts.map((p) => [p.id, p] as const));
  for (const p of [...delta.added, ...delta.updated]) byId.set(p.id, p);
  for (const id of delta.removed) byId.delete(id);
  return order.flatMap((id) => {
    const p = byId.get(id);
    return p ? [p] : [];
  });
}

/** Fold a delta into a host's state; also reports which parts to rebuild. */
export function applyEngineDelta(
  state: EngineState,
  delta: EngineDelta,
): { state: EngineState; changes: DeltaChanges } {
  return {
    state: {
      document: applyDocumentDelta(state.document, delta.document),
      parts: applyPartsDelta(state.parts, delta.parts),
    },
    changes: {
      rebuild: [...delta.parts.added, ...delta.parts.updated].map((p) => p.id),
      removed: [...delta.parts.removed],
    },
  };
}
