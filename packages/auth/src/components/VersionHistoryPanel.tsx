import { useCallback, useEffect, useState } from "react";
import {
  getVersionHistory,
  labelVersion,
  restoreVersion,
  unlabelVersion,
  type DocumentVersion,
} from "../version-history";

interface VersionHistoryPanelProps {
  /** Local document ID */
  localDocId: string;
  /** Cloud document ID (from syncStatus) */
  cloudDocId: string | null;
  /** Callback after restoring a version */
  onRestore?: () => void;
}

type Tab = "named" | "all";

/**
 * Panel showing version history for a cloud-synced document.
 *
 * Two tabs: **Named** (default) shows versions the user explicitly labeled as
 * waypoints ("v1 pre-review", "manufacturable"), and **All history** shows
 * every auto-saved row. CAD users need named waypoints, not 600 timestamps —
 * the default tab reflects that.
 *
 * Restoring a version pulls its content into the local IndexedDB doc and
 * triggers a sync back up to cloud.
 */
export function VersionHistoryPanel({
  localDocId,
  cloudDocId,
  onRestore,
}: VersionHistoryPanelProps) {
  const [versions, setVersions] = useState<DocumentVersion[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("named");
  const [labelingId, setLabelingId] = useState<string | null>(null);
  const [labelDraft, setLabelDraft] = useState("");

  const loadVersions = useCallback(async () => {
    if (!cloudDocId) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const v = await getVersionHistory(cloudDocId);
      setVersions(v);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, [cloudDocId]);

  useEffect(() => {
    void loadVersions();
  }, [loadVersions]);

  const handleRestore = async (version: DocumentVersion) => {
    setRestoring(version.id);
    try {
      await restoreVersion(localDocId, version);
      onRestore?.();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setRestoring(null);
    }
  };

  const handleLabelSubmit = async (versionId: string) => {
    const label = labelDraft.trim();
    if (!label) {
      setLabelingId(null);
      return;
    }
    try {
      await labelVersion(versionId, label);
      setLabelingId(null);
      setLabelDraft("");
      await loadVersions();
    } catch (err) {
      setError((err as Error).message);
    }
  };

  const handleUnlabel = async (versionId: string) => {
    try {
      await unlabelVersion(versionId);
      await loadVersions();
    } catch (err) {
      setError((err as Error).message);
    }
  };

  const formatRelativeTime = (timestamp: number): string => {
    const seconds = Math.floor((Date.now() - timestamp) / 1000);

    if (seconds < 60) return "just now";
    if (seconds < 3600) return `${Math.floor(seconds / 60)} minutes ago`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)} hours ago`;
    if (seconds < 604800) return `${Math.floor(seconds / 86400)} days ago`;

    return new Date(timestamp).toLocaleDateString();
  };

  // Not synced to cloud
  if (!cloudDocId) {
    return (
      <div className="p-4">
        <h3 className="font-semibold mb-3 text-zinc-900 dark:text-zinc-100">
          Version History
        </h3>
        <p className="text-sm text-zinc-500 dark:text-zinc-400">
          Enable cloud sync to access version history.
        </p>
        <p className="text-xs text-zinc-400 dark:text-zinc-500 mt-2">
          Your document changes will be automatically versioned when synced to
          the cloud.
        </p>
      </div>
    );
  }

  const namedVersions = versions.filter((v) => v.label);
  const shown = tab === "named" ? namedVersions : versions;

  return (
    <div className="p-4">
      <div className="flex items-center justify-between mb-3">
        <h3 className="font-semibold text-zinc-900 dark:text-zinc-100">
          Version History
        </h3>
        <div className="flex gap-1 text-xs">
          <button
            type="button"
            onClick={() => setTab("named")}
            className={
              tab === "named"
                ? "px-2 py-1 rounded bg-zinc-200 dark:bg-zinc-700 text-zinc-900 dark:text-zinc-100"
                : "px-2 py-1 rounded text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200"
            }
          >
            Named ({namedVersions.length})
          </button>
          <button
            type="button"
            onClick={() => setTab("all")}
            className={
              tab === "all"
                ? "px-2 py-1 rounded bg-zinc-200 dark:bg-zinc-700 text-zinc-900 dark:text-zinc-100"
                : "px-2 py-1 rounded text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200"
            }
          >
            All history
          </button>
        </div>
      </div>

      {loading && (
        <div className="flex items-center gap-2 text-sm text-zinc-500">
          <span className="animate-spin">&#8987;</span>
          Loading versions...
        </div>
      )}

      {error && (
        <div className="text-sm text-red-600 dark:text-red-400 mb-3">
          {error}
        </div>
      )}

      {!loading && shown.length === 0 && (
        <p className="text-sm text-zinc-500 dark:text-zinc-400">
          {tab === "named"
            ? "No named versions yet. Use “All history” and click “Label…” on a row to promote it to a named waypoint."
            : "No previous versions yet. Versions are created automatically when you save changes."}
        </p>
      )}

      {shown.length > 0 && (
        <div className="space-y-1">
          {shown.map((v) => (
            <div
              key={v.id}
              className="flex items-center justify-between py-2 px-2 -mx-2 rounded hover:bg-zinc-50 dark:hover:bg-zinc-800"
            >
              <div className="min-w-0 flex-1">
                {v.label ? (
                  <div className="text-sm font-medium text-zinc-900 dark:text-zinc-100 truncate">
                    {v.label}
                  </div>
                ) : (
                  <div className="text-sm font-medium text-zinc-900 dark:text-zinc-100">
                    Version {v.versionNumber}
                  </div>
                )}
                <div className="text-xs text-zinc-500 dark:text-zinc-400">
                  {v.label && `v${v.versionNumber} · `}
                  {formatRelativeTime(v.deviceModifiedAt)}
                </div>
              </div>
              <div className="flex items-center gap-2 shrink-0">
                {labelingId === v.id ? (
                  <>
                    <input
                      autoFocus
                      value={labelDraft}
                      onChange={(e) => setLabelDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void handleLabelSubmit(v.id);
                        if (e.key === "Escape") setLabelingId(null);
                      }}
                      placeholder="v1 pre-review"
                      className="text-xs px-1.5 py-0.5 border border-zinc-300 dark:border-zinc-600 rounded bg-white dark:bg-zinc-900 text-zinc-900 dark:text-zinc-100"
                    />
                    <button
                      type="button"
                      onClick={() => void handleLabelSubmit(v.id)}
                      className="text-xs text-blue-600 dark:text-blue-400"
                    >
                      Save
                    </button>
                  </>
                ) : v.label ? (
                  <button
                    type="button"
                    onClick={() => void handleUnlabel(v.id)}
                    className="text-xs text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200"
                    title="Remove label (auto-version row is preserved)"
                  >
                    Unlabel
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => {
                      setLabelingId(v.id);
                      setLabelDraft("");
                    }}
                    className="text-xs text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200"
                  >
                    Label…
                  </button>
                )}
                <button
                  onClick={() => handleRestore(v)}
                  disabled={restoring === v.id}
                  className="text-sm text-blue-600 hover:text-blue-700 dark:text-blue-400 dark:hover:text-blue-300 disabled:opacity-50"
                >
                  {restoring === v.id ? "Restoring..." : "Restore"}
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
