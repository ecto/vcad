import { useEffect } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { ClockCounterClockwise } from "@phosphor-icons/react/dist/ssr/ClockCounterClockwise";
import { GitBranch } from "@phosphor-icons/react/dist/ssr/GitBranch";
import { GitMerge } from "@phosphor-icons/react/dist/ssr/GitMerge";
import { useAuthStore } from "@vcad/auth";
import type { EntityChange, FieldChange, MergeConflict } from "@vcad/engine";
import {
  useVersionTimelineStore,
  conflictKey,
} from "@/stores/version-timeline-store";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Human-readable change rendering
// ---------------------------------------------------------------------------

const KIND_LABEL: Record<string, string> = {
  node: "feature",
  material: "material",
  "part-material": "material assignment",
  root: "part",
  "part-def": "part definition",
  instance: "instance",
  joint: "joint",
  parameter: "parameter",
  binding: "binding",
  clearance: "clearance assertion",
  doc: "document",
};

function entityLabel(c: EntityChange): string {
  const kind = KIND_LABEL[c.kind] ?? c.kind;
  return c.name ? `${kind} "${c.name}"` : `${kind} ${c.id}`;
}

function compactValue(v: unknown): string {
  if (v === null || v === undefined) return "∅";
  if (typeof v === "number") return String(Math.round(v * 1000) / 1000);
  if (typeof v === "string") return v.length > 24 ? `${v.slice(0, 24)}…` : v;
  const s = JSON.stringify(v);
  return s.length > 32 ? `${s.slice(0, 32)}…` : s;
}

function fieldLabel(f: FieldChange): string {
  // "op.size.0" reads better as "size.x" etc.; keep it simple and just
  // strip the leading "op." so change lines talk about the operation field.
  const path = f.path.replace(/^op\./, "") || "value";
  return `${path} ${compactValue(f.old)} → ${compactValue(f.new)}`;
}

function ChangeLine({ change }: { change: EntityChange }) {
  const label = entityLabel(change);
  if (change.type === "added") {
    return (
      <div className="text-[11px] leading-4">
        <span className="text-emerald-500">+ added</span>{" "}
        <span className="text-text">{label}</span>
      </div>
    );
  }
  if (change.type === "removed") {
    return (
      <div className="text-[11px] leading-4">
        <span className="text-red-400">− removed</span>{" "}
        <span className="text-text">{label}</span>
      </div>
    );
  }
  return (
    <div className="text-[11px] leading-4">
      <span className="text-amber-400">~ changed</span>{" "}
      <span className="text-text">{label}</span>
      <span className="text-text-muted">
        {" — "}
        {change.fields.slice(0, 3).map(fieldLabel).join("; ")}
        {change.fields.length > 3 ? ` (+${change.fields.length - 3} more)` : ""}
      </span>
    </div>
  );
}

function conflictLabel(c: MergeConflict): string {
  const kind = KIND_LABEL[c.kind] ?? c.kind;
  const entity = `${kind} ${c.id}`;
  switch (c.type) {
    case "both-added":
      return `${entity}: added on both sides with different content`;
    case "delete-modify":
      return `${entity}: deleted on ${c.deleted_by_ours ? "the original" : "this branch"}, modified on the other`;
    case "field":
      return `${entity} · ${c.path.replace(/^op\./, "")}: ${compactValue(c.ours)} (original) vs ${compactValue(c.theirs)} (branch)`;
  }
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

/**
 * Version timeline sidebar — Supabase `document_versions` with semantic
 * (feature-level) diffs, before/after viewport ghosting, restore-with-undo,
 * and branch → edit → three-way merge-back for shared docs.
 */
export function VersionTimeline() {
  const {
    versions,
    diffs,
    loading,
    error,
    cloudId,
    selectedVersionId,
    branchMeta,
    mergeBack,
    restoreUndo,
    closePanel,
    refresh,
    selectVersion,
    restore,
    undoRestore,
    branchFromVersion,
    startMergeBack,
    setResolution,
    cancelMergeBack,
  } = useVersionTimelineStore();
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  const isSignedIn = !!user && !isAnonymous;

  // Clear ghosting when the panel unmounts.
  useEffect(() => () => selectVersion(null), [selectVersion]);

  return (
    <div className="flex h-full w-full flex-col bg-surface border-l border-border select-none">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border">
        <ClockCounterClockwise size={14} className="text-brand shrink-0" />
        <span className="text-xs font-semibold text-text flex-1 truncate">
          Version timeline
        </span>
        <button
          onClick={() => void refresh()}
          className="p-1 text-text-muted hover:text-text transition-colors cursor-pointer"
          title="Refresh"
        >
          <ArrowsClockwise size={12} className={cn(loading && "animate-spin")} />
        </button>
        <button
          onClick={closePanel}
          className="p-1 text-text-muted hover:text-text transition-colors cursor-pointer"
          title="Close"
        >
          <X size={12} />
        </button>
      </div>

      {restoreUndo && (
        <div className="flex items-center gap-2 px-3 py-1.5 border-b border-border bg-surface-hover text-[11px] text-text-muted">
          <span className="flex-1 truncate">
            Restored — previous state saved
          </span>
          <button
            onClick={undoRestore}
            className="text-brand hover:underline cursor-pointer"
          >
            Undo restore
          </button>
        </div>
      )}

      {branchMeta && (
        <div className="px-3 py-2 border-b border-border space-y-1.5">
          <div className="flex items-center gap-1.5 text-[11px] text-text-muted">
            <GitBranch size={12} className="text-brand shrink-0" />
            <span className="truncate">
              Branch of &ldquo;{branchMeta.sourceName}&rdquo;
            </span>
          </div>
          {mergeBack && mergeBack.conflicts.length > 0 ? (
            <div className="space-y-1.5">
              <div className="text-[11px] font-medium text-amber-400">
                {mergeBack.conflicts.length} merge conflict
                {mergeBack.conflicts.length === 1 ? "" : "s"} — pick a side for
                each:
              </div>
              {mergeBack.conflicts.map((c) => {
                const key = conflictKey(c);
                const picked = mergeBack.resolutions[key];
                return (
                  <div key={key} className="rounded border border-border p-1.5 space-y-1">
                    <div className="text-[11px] text-text leading-4">
                      {conflictLabel(c)}
                    </div>
                    <div className="flex gap-1">
                      {(["ours", "theirs"] as const).map((side) => (
                        <button
                          key={side}
                          onClick={() => setResolution(key, side)}
                          className={cn(
                            "px-1.5 py-0.5 rounded text-[10px] border cursor-pointer transition-colors",
                            picked === side
                              ? "border-brand text-brand"
                              : "border-border text-text-muted hover:text-text",
                          )}
                        >
                          {side === "ours" ? "Keep original" : "Keep branch"}
                        </button>
                      ))}
                    </div>
                  </div>
                );
              })}
              <div className="flex gap-2">
                <button
                  onClick={() => void startMergeBack()}
                  disabled={
                    mergeBack.running ||
                    mergeBack.conflicts.some(
                      (c) => !mergeBack.resolutions[conflictKey(c)],
                    )
                  }
                  className="flex-1 px-2 py-1 rounded bg-brand text-white text-[11px] disabled:opacity-50 cursor-pointer"
                >
                  {mergeBack.running ? "Merging…" : "Merge with resolutions"}
                </button>
                <button
                  onClick={cancelMergeBack}
                  className="px-2 py-1 rounded border border-border text-[11px] text-text-muted cursor-pointer"
                >
                  Cancel
                </button>
              </div>
            </div>
          ) : (
            <button
              onClick={() => void startMergeBack()}
              disabled={mergeBack?.running}
              className="flex items-center justify-center gap-1.5 w-full px-2 py-1 rounded bg-brand text-white text-[11px] disabled:opacity-50 cursor-pointer"
            >
              <GitMerge size={12} />
              {mergeBack?.running
                ? "Merging…"
                : `Merge back into "${branchMeta.sourceName}"`}
            </button>
          )}
        </div>
      )}

      <div className="flex-1 overflow-y-auto">
        {!isSignedIn ? (
          <div className="p-4 text-xs text-text-muted">
            Sign in to enable cloud sync and version history.
          </div>
        ) : !cloudId && !loading ? (
          <div className="p-4 text-xs text-text-muted">
            This document isn&rsquo;t synced yet — versions appear after the
            first cloud sync.
          </div>
        ) : error ? (
          <div className="p-4 text-xs text-red-400">{error}</div>
        ) : versions.length === 0 && !loading ? (
          <div className="p-4 text-xs text-text-muted">No versions yet.</div>
        ) : (
          <ul>
            {versions.map((v, i) => {
              const diff = diffs[v.id];
              const selected = selectedVersionId === v.id;
              const isOldest = i === versions.length - 1;
              return (
                <li
                  key={v.id}
                  className={cn(
                    "border-b border-border px-3 py-2 cursor-pointer transition-colors",
                    selected ? "bg-surface-hover" : "hover:bg-surface-hover/50",
                  )}
                  onClick={() => selectVersion(v.id)}
                >
                  <div className="flex items-baseline gap-2">
                    <span className="text-xs font-medium text-text">
                      v{v.versionNumber}
                    </span>
                    {v.label && (
                      <span className="text-[10px] px-1 rounded bg-brand/15 text-brand">
                        {v.label}
                      </span>
                    )}
                    <span className="text-[10px] text-text-muted flex-1 text-right">
                      {new Date(v.createdAt).toLocaleString()}
                    </span>
                  </div>

                  <div className="mt-1 space-y-0.5">
                    {diff == null ? (
                      <div className="text-[11px] text-text-muted">
                        diff unavailable
                      </div>
                    ) : isOldest ? (
                      <div className="text-[11px] text-text-muted">
                        initial version
                      </div>
                    ) : diff.changes.length === 0 ? (
                      <div className="text-[11px] text-text-muted">
                        no semantic changes
                      </div>
                    ) : (
                      <>
                        {diff.changes
                          .slice(0, selected ? diff.changes.length : 3)
                          .map((c, j) => (
                            <ChangeLine key={j} change={c} />
                          ))}
                        {!selected && diff.changes.length > 3 && (
                          <div className="text-[10px] text-text-muted">
                            +{diff.changes.length - 3} more…
                          </div>
                        )}
                      </>
                    )}
                  </div>

                  {selected && (
                    <div
                      className="mt-1.5 flex gap-2"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <button
                        onClick={() => void restore(v.id)}
                        className="px-1.5 py-0.5 rounded border border-border text-[10px] text-text-muted hover:text-text cursor-pointer"
                      >
                        Restore
                      </button>
                      <button
                        onClick={() => void branchFromVersion(v.id)}
                        className="flex items-center gap-1 px-1.5 py-0.5 rounded border border-border text-[10px] text-text-muted hover:text-text cursor-pointer"
                      >
                        <GitBranch size={10} />
                        Branch
                      </button>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        {loading && (
          <div className="p-4 text-xs text-text-muted">Loading versions…</div>
        )}
      </div>
    </div>
  );
}
