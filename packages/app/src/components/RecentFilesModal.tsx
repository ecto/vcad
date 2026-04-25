/**
 * Recent Files modal — lightweight picker over the local recents list.
 *
 * Opens on the `vcad:open-recent-files` custom event. On click we read the
 * file via Tauri's fs plugin and route it through `processFile` via the
 * same `vcad:open-recent-file` event that App.tsx already handles for
 * drag-drop.
 */

import { useEffect, useState } from "react";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import {
  getRecentFiles,
  clearRecentFiles,
  readDocumentAtPath,
  type RecentFile,
} from "@/lib/native-file";
import { useNotificationStore } from "@/stores/notification-store";
import { t, tFmt } from "@vcad/core";
import { useLocaleStore } from "@/stores/locale-store";

export function RecentFilesModal() {
  const [open, setOpen] = useState(false);
  const [recents, setRecents] = useState<RecentFile[]>([]);
  useLocaleStore((s) => s.locale);

  useEffect(() => {
    const reload = () => setRecents(getRecentFiles());
    const openHandler = () => {
      reload();
      setOpen(true);
    };
    window.addEventListener("vcad:open-recent-files", openHandler);
    window.addEventListener("vcad:recent-files-changed", reload);
    return () => {
      window.removeEventListener("vcad:open-recent-files", openHandler);
      window.removeEventListener("vcad:recent-files-changed", reload);
    };
  }, []);

  const handlePick = async (file: RecentFile) => {
    const result = await readDocumentAtPath(file.path);
    if (!result) {
      useNotificationStore
        .getState()
        .addToast(tFmt("modal.recents.cant_open", { name: file.name }), "error");
      return;
    }
    // Hand back to the main pipeline as if the user had dropped this file.
    const pseudo = new File([result.contents], result.name);
    window.dispatchEvent(
      new CustomEvent("vcad:open-recent-file", { detail: { file: pseudo } }),
    );
    setOpen(false);
  };

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-24"
      onClick={() => setOpen(false)}
    >
      <div
        className="w-[480px] max-w-[90vw] rounded-md border border-border/60 bg-surface shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-border/40 px-3 py-2">
          <span className="text-xs font-medium text-text-muted">
            {t("modal.recents.title")}
          </span>
          {recents.length > 0 && (
            <button
              onClick={() => {
                clearRecentFiles();
                setRecents([]);
              }}
              className="flex items-center gap-1 text-[10px] text-text-muted hover:text-text"
              title={t("modal.recents.clear_tooltip")}
            >
              <Trash size={11} /> {t("modal.recents.clear")}
            </button>
          )}
        </div>
        {recents.length === 0 ? (
          <div className="px-3 py-6 text-center text-xs text-text-muted">
            {t("modal.recents.empty")}
          </div>
        ) : (
          <ul className="max-h-[60vh] overflow-y-auto">
            {recents.map((r) => (
              <li key={r.path}>
                <button
                  onClick={() => {
                    void handlePick(r);
                  }}
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-hover"
                >
                  <FolderOpen size={13} className="text-sky-400" />
                  <span className="flex-1 truncate">{r.name}</span>
                  <span className="truncate text-[10px] text-text-muted">
                    {r.path.replace(r.name, "").replace(/\/$/, "")}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
