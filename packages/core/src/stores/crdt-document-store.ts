/**
 * Thin CRDT-backed document store.
 *
 * Wraps WasmDocumentEngine from the Rust vcad-crdt crate. All document
 * mutations flow through 4 CRDT operations (create, delete, setParam, move).
 * The materializer in Rust produces the IR Document and PartInfo array.
 *
 * This store is the migration target — UI components should gradually
 * switch from `useDocumentStore` to `useCrdtDocumentStore`.
 */

import { create } from "zustand";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "../types.js";

/**
 * CRDT parameter value — matches Rust vcad_crdt::Value enum.
 */
export type CrdtValue =
  | { F64: number }
  | { Vec3: [number, number, number] }
  | { Bool: boolean }
  | { String: string }
  | { FeatureRef: string }
  | { FeatureRefList: string[] }
  | { Sketch: string };

/** Helper to create CrdtValue from a number */
export const f64 = (v: number): CrdtValue => ({ F64: v });
/** Helper to create CrdtValue from a 3D vector */
export const vec3 = (x: number, y: number, z: number): CrdtValue => ({
  Vec3: [x, y, z],
});
/** Helper to create CrdtValue from a boolean */
export const bool = (v: boolean): CrdtValue => ({ Bool: v });
/** Helper to create CrdtValue from a string */
export const str = (v: string): CrdtValue => ({ String: v });
/** Helper to create CrdtValue from a feature reference */
export const featureRef = (v: string): CrdtValue => ({ FeatureRef: v });

/** Result from a WASM mutation */
interface MutationResult {
  document: Document;
  parts: PartInfo[];
  createdFeatureId?: string;
}

/**
 * Interface for the WasmDocumentEngine from Rust.
 * This matches the #[wasm_bindgen] exports.
 */
interface WasmDocumentEngine {
  create_feature(kind: string, params_json: string): MutationResult;
  delete_feature(feature_id_json: string): MutationResult;
  set_param(
    feature_id_json: string,
    key: string,
    value_json: string,
  ): MutationResult;
  move_feature(feature_id_json: string, position_json: string): MutationResult;
  undo(): MutationResult;
  redo(): MutationResult;
  can_undo(): boolean;
  can_redo(): boolean;
  get_document_json(): string;
  get_parts_json(): string;
  get_ordered_features_json(): string;
  save(): Uint8Array;
  merge_remote(ops_json: string): MutationResult;
  get_sync_clock(): string;
  get_ops_since(remote_clock_json: string): string;
  free(): void;
}

/** Constructor for WasmDocumentEngine */
interface WasmDocumentEngineConstructor {
  new (): WasmDocumentEngine;
  load(bytes: Uint8Array): WasmDocumentEngine;
}

export interface CrdtDocumentState {
  engine: WasmDocumentEngine | null;
  document: Document | null;
  parts: PartInfo[];
  isDirty: boolean;
  documentId: string | null;
  documentName: string;

  /** Initialize the engine with the WASM module's WasmDocumentEngine class. */
  init: (EngineClass: WasmDocumentEngineConstructor) => void;

  /** Create a feature. Returns the feature ID. */
  createFeature: (
    kind: string,
    params: Record<string, CrdtValue>,
  ) => string | null;

  /** Delete a feature. */
  deleteFeature: (featureId: string) => void;

  /** Set a parameter on a feature. */
  setParam: (featureId: string, key: string, value: CrdtValue) => void;

  /** Move a feature to a new position. */
  moveFeature: (featureId: string, positionJson: string) => void;

  /** Undo the last action. */
  undo: () => void;

  /** Redo the last undone action. */
  redo: () => void;

  /** Whether undo is available. */
  canUndo: () => boolean;

  /** Whether redo is available. */
  canRedo: () => boolean;

  /** Get ordered features for the feature tree. */
  getOrderedFeatures: () => Array<{
    id: string;
    kind: string;
    params: Record<string, CrdtValue>;
  }>;

  /** Save the document to bytes. */
  save: () => Uint8Array | null;

  /** Load a document from bytes. */
  load: (
    bytes: Uint8Array,
    EngineClass: WasmDocumentEngineConstructor,
  ) => void;

  /** Merge remote operations (for collaboration). */
  mergeRemote: (opsJson: string) => void;

  /** Mark as clean (after save). */
  markClean: () => void;

  /** Set document metadata. */
  setDocumentName: (name: string) => void;
  setDocumentId: (id: string | null) => void;
}

export const useCrdtDocumentStore = create<CrdtDocumentState>((set, get) => ({
  engine: null,
  document: null,
  parts: [],
  isDirty: false,
  documentId: null,
  documentName: "Untitled",

  init: (EngineClass) => {
    const engine = new EngineClass();
    const docJson = engine.get_document_json();
    const partsJson = engine.get_parts_json();
    set({
      engine,
      document: JSON.parse(docJson),
      parts: JSON.parse(partsJson),
      isDirty: false,
    });
  },

  createFeature: (kind, params) => {
    const engine = get().engine;
    if (!engine) return null;

    const result = engine.create_feature(kind, JSON.stringify(params));
    set({
      document: result.document,
      parts: result.parts,
      isDirty: true,
    });
    return result.createdFeatureId ?? null;
  },

  deleteFeature: (featureId) => {
    const engine = get().engine;
    if (!engine) return;

    const result = engine.delete_feature(featureId);
    set({
      document: result.document,
      parts: result.parts,
      isDirty: true,
    });
  },

  setParam: (featureId, key, value) => {
    const engine = get().engine;
    if (!engine) return;

    const result = engine.set_param(featureId, key, JSON.stringify(value));
    set({
      document: result.document,
      parts: result.parts,
      isDirty: true,
    });
  },

  moveFeature: (featureId, positionJson) => {
    const engine = get().engine;
    if (!engine) return;

    const result = engine.move_feature(featureId, positionJson);
    set({
      document: result.document,
      parts: result.parts,
      isDirty: true,
    });
  },

  undo: () => {
    const engine = get().engine;
    if (!engine) return;

    const result = engine.undo();
    if (result) {
      set({ document: result.document, parts: result.parts });
    }
  },

  redo: () => {
    const engine = get().engine;
    if (!engine) return;

    const result = engine.redo();
    if (result) {
      set({ document: result.document, parts: result.parts });
    }
  },

  canUndo: () => get().engine?.can_undo() ?? false,
  canRedo: () => get().engine?.can_redo() ?? false,

  getOrderedFeatures: () => {
    const engine = get().engine;
    if (!engine) return [];
    return JSON.parse(engine.get_ordered_features_json());
  },

  save: () => {
    const engine = get().engine;
    if (!engine) return null;
    return engine.save();
  },

  load: (bytes, EngineClass) => {
    const engine = EngineClass.load(bytes);
    const docJson = engine.get_document_json();
    const partsJson = engine.get_parts_json();
    set({
      engine,
      document: JSON.parse(docJson),
      parts: JSON.parse(partsJson),
      isDirty: false,
    });
  },

  mergeRemote: (opsJson) => {
    const engine = get().engine;
    if (!engine) return;

    const result = engine.merge_remote(opsJson);
    set({
      document: result.document,
      parts: result.parts,
    });
  },

  markClean: () => set({ isDirty: false }),
  setDocumentName: (name) => set({ documentName: name }),
  setDocumentId: (id) => set({ documentId: id }),
}));
