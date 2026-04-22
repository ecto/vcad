/**
 * Runtime capability detection.
 *
 * The desktop (Tauri) and browser builds of vcad render the same UI, but
 * different backends are reachable in each environment. Features should
 * consult this layer rather than sprinkling `isTauri()` checks through the
 * component tree.
 */

import { useEffect, useState } from "react";
import { isTauri, invoke } from "@/lib/tauri";

export interface LocalAi {
  /** Ollama reachable at 127.0.0.1:11434 */
  ollama: boolean;
  /** The probed endpoint, if any */
  endpoint: string | null;
  /** Models discovered at that endpoint */
  models: string[];
}

export interface Capabilities {
  /** Running inside the Tauri desktop shell */
  tauri: boolean;
  /** Native Bambu printer bridge available (always true in desktop) */
  bambu: boolean;
  /** Local AI discovery results */
  localAi: LocalAi;
  /** Native filesystem (open/save dialogs, recent files) */
  nativeFs: boolean;
}

const NULL_CAPABILITIES: Capabilities = {
  tauri: false,
  bambu: false,
  localAi: { ollama: false, endpoint: null, models: [] },
  nativeFs: false,
};

let cache: Capabilities | null = null;
let inflight: Promise<Capabilities> | null = null;

interface LocalAiProbeResult {
  ollama_url: string | null;
  models: string[];
}

async function probeLocalAi(): Promise<LocalAi> {
  if (!isTauri()) return NULL_CAPABILITIES.localAi;
  try {
    const result = await invoke<LocalAiProbeResult>("local_ai_probe");
    return {
      ollama: result.ollama_url !== null,
      endpoint: result.ollama_url,
      models: result.models,
    };
  } catch {
    return NULL_CAPABILITIES.localAi;
  }
}

export async function probeCapabilities(): Promise<Capabilities> {
  if (cache) return cache;
  if (inflight) return inflight;
  inflight = (async () => {
    const tauri = isTauri();
    const localAi = await probeLocalAi();
    const caps: Capabilities = {
      tauri,
      bambu: tauri,
      localAi,
      nativeFs: false,
    };
    cache = caps;
    inflight = null;
    return caps;
  })();
  return inflight;
}

/** React hook — resolves on mount, returns null capabilities until probed. */
export function useCapabilities(): Capabilities {
  const [caps, setCaps] = useState<Capabilities>(cache ?? NULL_CAPABILITIES);
  useEffect(() => {
    if (cache) return;
    let alive = true;
    probeCapabilities().then((c) => {
      if (alive) setCaps(c);
    });
    return () => {
      alive = false;
    };
  }, []);
  return caps;
}
