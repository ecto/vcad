/**
 * Native file I/O via Tauri plugins.
 *
 * Browser build keeps using blob download + <input type="file">; Tauri
 * build gets real Save/Open dialogs and filesystem access. Every export
 * here is a no-op outside Tauri — callers can call unconditionally and
 * check the return value.
 */

import { isTauri } from "@/lib/tauri";

const RECENT_FILES_KEY = "vcad:recentFiles";
const RECENT_FILES_MAX = 10;

export interface RecentFile {
  path: string;
  name: string;
  openedAt: number;
}

/** Return the persisted recent files list. Most-recent first. */
export function getRecentFiles(): RecentFile[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(RECENT_FILES_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (r): r is RecentFile =>
        typeof r?.path === "string" && typeof r?.name === "string",
    );
  } catch {
    return [];
  }
}

/** Add / promote `path` to the top of the recent files list. */
export function addRecentFile(path: string): void {
  if (typeof localStorage === "undefined") return;
  const name = path.split("/").pop() ?? path;
  const entry: RecentFile = { path, name, openedAt: Date.now() };
  const current = getRecentFiles().filter((r) => r.path !== path);
  const next = [entry, ...current].slice(0, RECENT_FILES_MAX);
  try {
    localStorage.setItem(RECENT_FILES_KEY, JSON.stringify(next));
    window.dispatchEvent(new CustomEvent("vcad:recent-files-changed"));
  } catch {
    // Quota issues are advisory — drop silently.
  }
}

export function clearRecentFiles(): void {
  if (typeof localStorage === "undefined") return;
  localStorage.removeItem(RECENT_FILES_KEY);
  window.dispatchEvent(new CustomEvent("vcad:recent-files-changed"));
}

/**
 * Show a native save dialog for the current document, write the file,
 * record it in recents, and return the chosen path. Returns null if the
 * user cancels or we're not running under Tauri.
 */
export async function saveDocumentNative(
  contents: string,
  defaultName = "document.vcad",
): Promise<string | null> {
  if (!isTauri()) return null;
  const { save } = await import("@tauri-apps/plugin-dialog");
  const { writeTextFile } = await import("@tauri-apps/plugin-fs");
  const path = await save({
    defaultPath: defaultName,
    filters: [{ name: "vcad", extensions: ["vcad"] }],
  });
  if (!path) return null;
  await writeTextFile(path, contents);
  addRecentFile(path);
  return path;
}

/**
 * Show a native open dialog, read the picked file's text contents, record
 * the path in recents, and return `{ path, contents }`. Returns null on
 * cancel or outside Tauri.
 */
export async function openDocumentNative(): Promise<
  { path: string; contents: string; name: string } | null
> {
  if (!isTauri()) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const { readTextFile } = await import("@tauri-apps/plugin-fs");
  const picked = await open({
    multiple: false,
    directory: false,
    filters: [
      { name: "vcad", extensions: ["vcad", "loon", "json"] },
      { name: "STEP", extensions: ["step", "stp"] },
      { name: "STL", extensions: ["stl"] },
    ],
  });
  if (!picked || typeof picked !== "string") return null;
  const contents = await readTextFile(picked);
  addRecentFile(picked);
  const name = picked.split("/").pop() ?? picked;
  return { path: picked, contents, name };
}

/** Read a file at a previously-recorded path (e.g. from the recents list). */
export async function readDocumentAtPath(
  path: string,
): Promise<{ contents: string; name: string } | null> {
  if (!isTauri()) return null;
  try {
    const { readTextFile } = await import("@tauri-apps/plugin-fs");
    const contents = await readTextFile(path);
    const name = path.split("/").pop() ?? path;
    return { contents, name };
  } catch (err) {
    console.warn(`[native-file] failed to read ${path}:`, err);
    return null;
  }
}
