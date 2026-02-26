/**
 * Tauri desktop integration layer.
 *
 * Detects whether the app is running inside Tauri and provides
 * typed wrappers around desktop-only commands. Falls back gracefully
 * in the browser (no-ops or browser equivalents).
 */

// ---------------------------------------------------------------------------
// Environment detection
// ---------------------------------------------------------------------------

declare global {
  interface Window {
    __TAURI_INTERNALS__?: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
    };
  }
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && window.__TAURI_INTERNALS__ != null;
}

// ---------------------------------------------------------------------------
// Generic invoke
// ---------------------------------------------------------------------------

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error(`Tauri not available (attempted invoke: ${cmd})`);
  }
  return window.__TAURI_INTERNALS__!.invoke<T>(cmd, args);
}

// ---------------------------------------------------------------------------
// Typed command wrappers
// ---------------------------------------------------------------------------

export interface PrinterInfo {
  id: string;
  name: string;
  printer_type: string;
  connected: boolean;
}

export interface PlatformInfo {
  os: string;
  arch: string;
  desktop: boolean;
}

export interface RecentFile {
  path: string;
  name: string;
  modified: number;
}

export interface FileFilter {
  name: string;
  extensions: string[];
}

// -- Printer --

export async function discoverPrinters(): Promise<PrinterInfo[]> {
  if (!isTauri()) return [];
  return invoke<PrinterInfo[]>("discover_printers");
}

export async function sendToPrinter(printerId: string, gcode: string): Promise<void> {
  return invoke("send_to_printer", { printerId, gcode });
}

// -- Files --

export async function openNativeFileDialog(filters: FileFilter[]): Promise<string | null> {
  if (!isTauri()) return null;
  return invoke<string | null>("open_native_file_dialog", { filters });
}

export async function readFileBytes(path: string): Promise<number[]> {
  return invoke<number[]>("read_file_bytes", { path });
}

export async function writeFileBytes(path: string, data: number[]): Promise<void> {
  return invoke("write_file_bytes", { path, data });
}

export async function getRecentFiles(): Promise<RecentFile[]> {
  if (!isTauri()) return [];
  return invoke<RecentFile[]>("get_recent_files");
}

export async function launchExternalSlicer(stlPath: string): Promise<void> {
  return invoke("launch_external_slicer", { stlPath });
}

// -- System --

export async function isDesktop(): Promise<boolean> {
  if (!isTauri()) return false;
  return invoke<boolean>("is_desktop");
}

export async function getPlatformInfo(): Promise<PlatformInfo | null> {
  if (!isTauri()) return null;
  return invoke<PlatformInfo>("get_platform_info");
}
