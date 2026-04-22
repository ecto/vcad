/**
 * Printer bridge client.
 *
 * Two transports, selected at runtime:
 *   - Desktop (Tauri): invokes Rust commands that talk directly to the
 *     Bambu printer over MQTT + FTPS. Always available in desktop.
 *   - Browser: talks to a local HTTP relay at 127.0.0.1:7878. If the relay
 *     isn't running, callers should fall back to "Download 3MF".
 *
 * The public surface is identical across transports so the rest of the
 * app (components/print/*, stores/printer-store.ts) doesn't need to care.
 */

import { isTauri, invoke } from "@/lib/tauri";

const DEFAULT_RELAY_URL = "http://127.0.0.1:7878";

export interface RelayPrinterInfo {
  ip: string;
  serial: string;
  model: string;
  name: string;
}

export interface RelayStatus {
  state: string;
  progress_percent: number;
  layer_current: number;
  layer_total: number;
  time_remaining_min: number;
  nozzle_temp: number;
  nozzle_target: number;
  bed_temp: number;
  bed_target: number;
  fan_speed: number;
  filename: string | null;
}

let relayUrl = DEFAULT_RELAY_URL;

/** Set the relay URL (for testing or custom configurations). */
export function setRelayUrl(url: string) {
  relayUrl = url;
}

/** Check if a printer bridge is reachable. */
export async function isRelayAvailable(): Promise<boolean> {
  if (isTauri()) return true;
  try {
    const res = await fetch(`${relayUrl}/health`, {
      signal: AbortSignal.timeout(2000),
    });
    return res.ok;
  } catch {
    return false;
  }
}

/** Discover printers on the local network. */
export async function discoverPrinters(): Promise<RelayPrinterInfo[]> {
  if (isTauri()) {
    return invoke<RelayPrinterInfo[]>("bambu_discover");
  }
  const res = await fetch(`${relayUrl}/printers`);
  if (!res.ok) throw new Error(`Discovery failed: ${res.statusText}`);
  return res.json();
}

/** Connect to a printer. */
export async function connectPrinter(
  ip: string,
  serial: string,
  accessCode: string
): Promise<void> {
  if (isTauri()) {
    await invoke<void>("bambu_connect", { ip, serial, accessCode });
    return;
  }
  const res = await fetch(`${relayUrl}/connect`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ip, serial, access_code: accessCode }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Connect failed: ${text}`);
  }
}

/** Get current printer status. */
export async function getPrinterStatus(): Promise<RelayStatus> {
  if (isTauri()) {
    return invoke<RelayStatus>("bambu_status");
  }
  const res = await fetch(`${relayUrl}/status`);
  if (!res.ok) throw new Error(`Status failed: ${res.statusText}`);
  return res.json();
}

/** Send a 3MF file to the printer. */
export async function sendPrint(
  data: Uint8Array,
  filename?: string
): Promise<void> {
  // Convert to base64 (shared by both transports).
  let binary = "";
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]!);
  }
  const dataBase64 = btoa(binary);

  if (isTauri()) {
    await invoke<void>("bambu_send_print", {
      dataBase64,
      filename: filename ?? "vcad_print.3mf",
    });
    return;
  }
  const res = await fetch(`${relayUrl}/print`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      data_base64: dataBase64,
      filename: filename || "vcad_print.3mf",
    }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Print failed: ${text}`);
  }
}

/** Send a control command (pause/resume/stop). */
export async function controlPrinter(
  action: "pause" | "resume" | "stop"
): Promise<void> {
  if (isTauri()) {
    await invoke<void>("bambu_control", { action });
    return;
  }
  const res = await fetch(`${relayUrl}/control`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ action }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Control failed: ${text}`);
  }
}
