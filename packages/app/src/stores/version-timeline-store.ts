import { create } from "zustand";
import type { Document } from "@vcad/ir";
import {
  semanticDiff,
  threeWayMerge,
  mergeAvailable,
  type DocumentDiff,
  type MergeConflict,
  type MergeResolution,
  type TriangleMesh,
} from "@vcad/engine";
import {
  useDocumentStore,
  useEngineStore,
  getKernelWasmSync,
  type VcadFile,
} from "@vcad/core";
import {
  getVersionHistory,
  restoreVersion,
  cloudContentToVcadFile,
  triggerSync,
  type DocumentVersion,
} from "@vcad/auth";
import {
  loadDocument as loadStoredDocument,
  saveDocument as saveStoredDocument,
  updateDocument as updateStoredDocument,
} from "@/lib/storage";
import { newDocId } from "@/lib/doc-id";
import { useNotificationStore } from "@/stores/notification-store";

/** Ghost geometry for the before/after overlay. */
export interface VersionGhost {
  /** Geometry present in the parent version but gone (or changed) — red. */
  removed: TriangleMesh[];
  /** Geometry new (or changed) in the selected version — green. */
  added: TriangleMesh[];
}

/** Branch bookkeeping persisted per branch document in localStorage. */
interface BranchMeta {
  /** Local id of the document the branch was forked from. */
  sourceLocalId: string;
  /** Cloud id of the source document (where versions live). */
  sourceCloudId: string;
  /** `document_versions.id` of the fork point — the merge base. */
  baseVersionId: string;
  /** Display name of the source document. */
  sourceName: string;
}

/** In-flight merge-back state surfaced to the conflict-resolution UI. */
export interface MergeBackState {
  conflicts: MergeConflict[];
  /** User picks keyed by `${kind} ${id} ${path ?? ""}`. */
  resolutions: Record<string, "ours" | "theirs">;
  running: boolean;
}

const BRANCH_META_KEY = "vcad-branch-meta";

function readBranchMeta(): Record<string, BranchMeta> {
  try {
    return JSON.parse(localStorage.getItem(BRANCH_META_KEY) ?? "{}");
  } catch {
    return {};
  }
}

function writeBranchMeta(all: Record<string, BranchMeta>): void {
  localStorage.setItem(BRANCH_META_KEY, JSON.stringify(all));
}

/** Minimal structural view of the WASM document engine used for scratch work. */
interface ScratchEngine {
  get_document_json(): string;
  save(): Uint8Array;
  free(): void;
}
interface ScratchEngineClass {
  load(bytes: Uint8Array): ScratchEngine;
  from_v1_json(json: string): ScratchEngine;
}

function scratchEngineClass(): ScratchEngineClass | null {
  const cls = (
    useDocumentStore.getState() as unknown as {
      _crdtEngineClass?: ScratchEngineClass;
    }
  )._crdtEngineClass;
  return cls ?? null;
}

/**
 * Materialize a `VcadFile` into an IR `Document` on a throwaway engine —
 * the live `_crdtEngine` is never touched.
 */
function vcadFileToIr(file: VcadFile): Document | null {
  if (file.kind === "loon" || file.kind === "legacy") {
    // Loon/legacy files carry the evaluated Document inline (cloud loon rows
    // don't — those fall through to the engine path below when absent).
    const inline = (file as { document?: Document }).document;
    if (inline) return inline;
  }
  const cls = scratchEngineClass();
  if (!cls) return null;
  let engine: ScratchEngine | null = null;
  try {
    engine =
      file.kind === "crdt"
        ? cls.load(file.crdtBytes)
        : cls.from_v1_json(JSON.stringify((file as { document?: Document }).document ?? {}));
    return JSON.parse(engine.get_document_json()) as Document;
  } catch (e) {
    console.warn("[version-timeline] failed to materialize version:", e);
    return null;
  } finally {
    try {
      engine?.free();
    } catch {
      /* scratch engine — best effort */
    }
  }
}

/** Materialize a cloud `document_versions.content` blob into an IR Document. */
function versionToIr(version: DocumentVersion): Document | null {
  const file = cloudContentToVcadFile(version.content) as VcadFile;
  if (!file || typeof file !== "object" || !("kind" in file)) {
    // Raw untagged content the sniffer didn't recognize.
    const raw = version.content as { nodes?: unknown; roots?: unknown };
    return raw?.nodes && raw?.roots ? (version.content as Document) : null;
  }
  return vcadFileToIr(file);
}

/** Turn an IR Document into a saveable CRDT VcadFile via a scratch engine. */
function irToVcadFile(doc: Document): VcadFile | null {
  const cls = scratchEngineClass();
  if (!cls) return null;
  let engine: ScratchEngine | null = null;
  try {
    engine = cls.from_v1_json(JSON.stringify(doc));
    return { kind: "crdt", version: "0.4", crdtBytes: engine.save() };
  } catch (e) {
    console.warn("[version-timeline] failed to encode document:", e);
    return null;
  } finally {
    try {
      engine?.free();
    } catch {
      /* scratch engine — best effort */
    }
  }
}

/** Cheap structural fingerprint to detect changed geometry between versions. */
function meshFingerprint(mesh: TriangleMesh): string {
  const p = mesh.positions;
  const n = p.length;
  let sample = 0;
  for (let i = 0; i < n; i += Math.max(1, Math.floor(n / 32))) {
    sample = (sample * 31 + Math.round((p[i] ?? 0) * 1000)) | 0;
  }
  return `${n}:${mesh.indices.length}:${sample}`;
}

/**
 * Evaluate two versions and split their geometry into removed (red) / added
 * (green) ghost meshes, matching parts by scene-root id.
 */
function computeGhost(parent: Document | null, selected: Document): VersionGhost | null {
  const engine = useEngineStore.getState().engine;
  if (!engine) return null;
  const evalRoots = (doc: Document): Map<string, TriangleMesh> => {
    const out = new Map<string, TriangleMesh>();
    try {
      const scene = engine.evaluate(doc);
      const visible = doc.roots.filter((r) => r.visible !== false);
      scene.parts.forEach((p, i) => {
        const root = visible[i];
        if (root && p.mesh.positions.length > 0) out.set(String(root.root), p.mesh);
      });
    } catch (e) {
      console.warn("[version-timeline] ghost evaluation failed:", e);
    }
    return out;
  };
  const before = parent ? evalRoots(parent) : new Map<string, TriangleMesh>();
  const after = evalRoots(selected);
  const removed: TriangleMesh[] = [];
  const added: TriangleMesh[] = [];
  for (const [id, mesh] of before) {
    const counterpart = after.get(id);
    if (!counterpart) removed.push(mesh);
    else if (meshFingerprint(counterpart) !== meshFingerprint(mesh)) removed.push(mesh);
  }
  for (const [id, mesh] of after) {
    const counterpart = before.get(id);
    if (!counterpart || meshFingerprint(counterpart) !== meshFingerprint(mesh))
      added.push(mesh);
  }
  if (removed.length === 0 && added.length === 0) return null;
  return { removed, added };
}

interface VersionTimelineState {
  open: boolean;
  loading: boolean;
  error: string | null;
  /** Cloud doc id of the open document; null when not synced. */
  cloudId: string | null;
  /** Versions newest-first, as returned by `getVersionHistory`. */
  versions: DocumentVersion[];
  /** Per-version semantic diff vs its parent (the next-older version). */
  diffs: Record<string, DocumentDiff>;
  selectedVersionId: string | null;
  ghost: VersionGhost | null;
  /** Set when the open document is a branch of another document. */
  branchMeta: BranchMeta | null;
  mergeBack: MergeBackState | null;
  /** Snapshot for undoing the last restore. */
  restoreUndo: { file: VcadFile; label: string } | null;

  openPanel: () => Promise<void>;
  closePanel: () => void;
  refresh: () => Promise<void>;
  selectVersion: (versionId: string | null) => void;
  restore: (versionId: string) => Promise<void>;
  undoRestore: () => void;
  branchFromVersion: (versionId: string) => Promise<void>;
  startMergeBack: () => Promise<void>;
  setResolution: (conflictKey: string, side: "ours" | "theirs") => void;
  cancelMergeBack: () => void;
}

const toast = (msg: string, kind: "success" | "error" | "info" = "info") =>
  useNotificationStore.getState().addToast(msg, kind);

/** Stable key for one conflict (also used to attach a user resolution). */
export function conflictKey(c: MergeConflict): string {
  return `${c.kind} ${c.id} ${c.type === "field" ? c.path : ""}`;
}

export const useVersionTimelineStore = create<VersionTimelineState>((set, get) => {
  /** IR cache per version id, session-scoped. */
  const irCache = new Map<string, Document | null>();

  const versionIr = (v: DocumentVersion): Document | null => {
    if (!irCache.has(v.id)) irCache.set(v.id, versionToIr(v));
    return irCache.get(v.id) ?? null;
  };

  const loadVersions = async () => {
    const { documentId } = useDocumentStore.getState();
    if (!documentId) {
      set({ versions: [], cloudId: null, loading: false });
      return;
    }
    set({ loading: true, error: null });
    const branchMeta = readBranchMeta()[documentId] ?? null;
    try {
      const stored = await loadStoredDocument(documentId);
      const cloudId = stored?.cloudId ?? null;
      if (!cloudId) {
        set({ versions: [], cloudId: null, loading: false, branchMeta });
        return;
      }
      const versions = await getVersionHistory(cloudId);
      irCache.clear();
      // Diff each version against its parent (next entry — list is
      // newest-first). Computed eagerly: histories are capped server-side
      // and the diff is entity-level, not geometric.
      const diffs: Record<string, DocumentDiff> = {};
      const kernel = (() => {
        try {
          return getKernelWasmSync();
        } catch {
          return null;
        }
      })();
      for (let i = 0; i < versions.length; i++) {
        const row = versions[i];
        if (!row) continue;
        const cur = versionIr(row);
        if (!cur) continue;
        const parentRow = versions[i + 1];
        const parent = parentRow ? versionIr(parentRow) : null;
        try {
          diffs[row.id] = semanticDiff(
            parent ?? ({ nodes: {}, roots: [], materials: {}, part_materials: {} } as unknown as Document),
            cur,
            kernel as Parameters<typeof semanticDiff>[2],
          );
        } catch (e) {
          console.warn("[version-timeline] diff failed:", e);
        }
      }
      set({ versions, diffs, cloudId, loading: false, branchMeta });
    } catch (e) {
      set({ error: String(e), loading: false, branchMeta });
    }
  };

  return {
    open: false,
    loading: false,
    error: null,
    cloudId: null,
    versions: [],
    diffs: {},
    selectedVersionId: null,
    ghost: null,
    branchMeta: null,
    mergeBack: null,
    restoreUndo: null,

    openPanel: async () => {
      set({ open: true });
      await loadVersions();
    },

    closePanel: () =>
      set({ open: false, selectedVersionId: null, ghost: null, mergeBack: null }),

    refresh: loadVersions,

    selectVersion: (versionId) => {
      if (!versionId || versionId === get().selectedVersionId) {
        set({ selectedVersionId: null, ghost: null });
        return;
      }
      const { versions } = get();
      const idx = versions.findIndex((v) => v.id === versionId);
      const row = versions[idx];
      if (idx < 0 || !row) return;
      const selected = versionIr(row);
      if (!selected) {
        set({ selectedVersionId: versionId, ghost: null });
        return;
      }
      const parentRow = versions[idx + 1];
      const parent = parentRow ? versionIr(parentRow) : null;
      set({ selectedVersionId: versionId, ghost: computeGhost(parent, selected) });
    },

    restore: async (versionId) => {
      const { versions } = get();
      const version = versions.find((v) => v.id === versionId);
      const { documentId } = useDocumentStore.getState();
      if (!version || !documentId) return;
      try {
        // Snapshot the current state first so the restore is undoable.
        const current = await loadStoredDocument(documentId);
        await restoreVersion(documentId, version);
        const stored = await loadStoredDocument(documentId);
        if (stored) {
          useDocumentStore.getState().loadDocument(stored.document);
        }
        set({
          restoreUndo: current
            ? { file: current.document, label: `v${version.versionNumber}` }
            : null,
          selectedVersionId: null,
          ghost: null,
        });
        toast(`Restored version ${version.versionNumber}`, "success");
        await loadVersions();
      } catch (e) {
        toast(`Restore failed: ${e}`, "error");
      }
    },

    undoRestore: () => {
      const undo = get().restoreUndo;
      const { documentId, documentName } = useDocumentStore.getState();
      if (!undo || !documentId) return;
      useDocumentStore.getState().loadDocument(undo.file);
      void saveStoredDocument(documentId, documentName ?? "document", undo.file)
        .then(() => updateStoredDocument(documentId, { syncStatus: "pending" }))
        .then(() => triggerSync());
      set({ restoreUndo: null });
      toast("Restore undone", "success");
    },

    branchFromVersion: async (versionId) => {
      const { versions, cloudId } = get();
      const version = versions.find((v) => v.id === versionId);
      const { documentId, documentName } = useDocumentStore.getState();
      if (!version || !documentId || !cloudId) return;
      const file = cloudContentToVcadFile(version.content) as VcadFile;
      if (!file || typeof file !== "object" || !("kind" in file)) {
        toast("This version cannot be branched (unrecognized content)", "error");
        return;
      }
      const branchId = newDocId();
      const branchName = `${documentName ?? "document"} (branch of v${version.versionNumber})`;
      await saveStoredDocument(branchId, branchName, file);
      const all = readBranchMeta();
      all[branchId] = {
        sourceLocalId: documentId,
        sourceCloudId: cloudId,
        baseVersionId: version.id,
        sourceName: documentName ?? "document",
      };
      writeBranchMeta(all);
      useDocumentStore.getState().loadDocument(file);
      useDocumentStore.getState().setDocumentMeta(branchId, branchName);
      toast(`Branched from v${version.versionNumber} — merge back when ready`, "success");
      await loadVersions();
    },

    startMergeBack: async () => {
      const { branchMeta, mergeBack } = get();
      const { documentId, document: oursCurrent } = useDocumentStore.getState();
      if (!branchMeta || !documentId) return;
      const kernel = (() => {
        try {
          return getKernelWasmSync();
        } catch {
          return null;
        }
      })();
      if (!mergeAvailable(kernel as Parameters<typeof mergeAvailable>[0])) {
        toast("Merge needs a newer kernel build (documentMerge binding missing)", "error");
        return;
      }
      set({
        mergeBack: {
          conflicts: mergeBack?.conflicts ?? [],
          resolutions: mergeBack?.resolutions ?? {},
          running: true,
        },
      });
      try {
        // base: the fork-point version; ours: source doc's current state;
        // theirs: this branch's current state.
        const history = await getVersionHistory(branchMeta.sourceCloudId);
        const baseVersion = history.find((v) => v.id === branchMeta.baseVersionId);
        const base = baseVersion ? versionToIr(baseVersion) : null;
        const sourceStored = await loadStoredDocument(branchMeta.sourceLocalId);
        const ours = sourceStored ? vcadFileToIr(sourceStored.document) : null;
        if (!base || !ours) {
          toast("Merge base or source document unavailable", "error");
          set({ mergeBack: null });
          return;
        }
        const resolutions: MergeResolution[] = (mergeBack?.conflicts ?? [])
          .map((c) => {
            const side = mergeBack?.resolutions[conflictKey(c)];
            if (!side) return null;
            return {
              kind: c.kind,
              id: c.id,
              ...(c.type === "field" ? { path: c.path } : {}),
              side,
            } as MergeResolution;
          })
          .filter((r): r is MergeResolution => r !== null);
        const outcome = threeWayMerge(
          base,
          ours,
          oursCurrent,
          resolutions,
          kernel as Parameters<typeof threeWayMerge>[4],
        );
        if (outcome.conflicts) {
          set({
            mergeBack: {
              conflicts: outcome.conflicts,
              resolutions: mergeBack?.resolutions ?? {},
              running: false,
            },
          });
          return;
        }
        const mergedFile = irToVcadFile(outcome.merged);
        if (!mergedFile) {
          toast("Failed to encode merged document", "error");
          set({ mergeBack: { conflicts: [], resolutions: {}, running: false } });
          return;
        }
        await saveStoredDocument(
          branchMeta.sourceLocalId,
          sourceStored?.name ?? branchMeta.sourceName,
          mergedFile,
        );
        await updateStoredDocument(branchMeta.sourceLocalId, { syncStatus: "pending" });
        triggerSync();
        // Open the merged source document.
        useDocumentStore.getState().loadDocument(mergedFile);
        useDocumentStore
          .getState()
          .setDocumentMeta(branchMeta.sourceLocalId, sourceStored?.name ?? branchMeta.sourceName);
        const all = readBranchMeta();
        delete all[documentId];
        writeBranchMeta(all);
        set({ mergeBack: null, branchMeta: null });
        toast(`Merged branch back into "${branchMeta.sourceName}"`, "success");
        await loadVersions();
      } catch (e) {
        toast(`Merge failed: ${e}`, "error");
        set({
          mergeBack: {
            conflicts: mergeBack?.conflicts ?? [],
            resolutions: mergeBack?.resolutions ?? {},
            running: false,
          },
        });
      }
    },

    setResolution: (key, side) =>
      set((s) =>
        s.mergeBack
          ? {
              mergeBack: {
                ...s.mergeBack,
                resolutions: { ...s.mergeBack.resolutions, [key]: side },
              },
            }
          : {},
      ),

    cancelMergeBack: () => set({ mergeBack: null }),
  };
});
