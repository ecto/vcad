/**
 * Print relay client for communicating with the local print server.
 *
 * The web app cannot directly talk to Bambu printers (MQTT/FTPS).
 * Instead, it talks to the local relay at http://127.0.0.1:7878.
 *
 * If the relay is not running, the app gracefully falls back to
 * "Download 3MF" instead of "Print".
 */

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

/** Check if the relay server is running. */
export async function isRelayAvailable(): Promise<boolean> {
  try {
    const res = await fetch(`${relayUrl}/health`, {
      signal: AbortSignal.timeout(2000),
    });
    return res.ok;
  } catch {
    return false;
  }
}

/** Discover printers on the local network via the relay. */
export async function discoverPrinters(): Promise<RelayPrinterInfo[]> {
  const res = await fetch(`${relayUrl}/printers`);
  if (!res.ok) throw new Error(`Discovery failed: ${res.statusText}`);
  return res.json();
}

/** Connect to a printer via the relay. */
export async function connectPrinter(
  ip: string,
  serial: string,
  accessCode: string
): Promise<void> {
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

/** Get current printer status via the relay. */
export async function getPrinterStatus(): Promise<RelayStatus> {
  const res = await fetch(`${relayUrl}/status`);
  if (!res.ok) throw new Error(`Status failed: ${res.statusText}`);
  return res.json();
}

/** Send a 3MF file to the printer via the relay. */
export async function sendPrint(
  data: Uint8Array,
  filename?: string
): Promise<void> {
  // Convert to base64
  let binary = "";
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]!);
  }
  const dataBase64 = btoa(binary);

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

/** Send a control command (pause/resume/stop) via the relay. */
export async function controlPrinter(
  action: "pause" | "resume" | "stop"
): Promise<void> {
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
