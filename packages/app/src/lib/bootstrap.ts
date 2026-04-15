import {
  Engine,
  useDocumentStore,
  useEngineStore,
  useUiStore,
  logger,
  commandRegistry,
} from "@vcad/core";
import { initializeGpu, initializeRayTracer } from "@vcad/engine";
import { registerSW } from "virtual:pwa-register";

import { useBootStore } from "@/stores/boot-store";
import {
  getMostRecentDocument,
  loadDocument as loadDocumentFromDb,
  generateDocumentName,
} from "@/lib/storage";
import { loadDocumentFromUrl } from "@/lib/url-document";
import { useNotificationStore } from "@/stores/notification-store";

/**
 * Resolve the kernel WASM URL through Vite's asset pipeline. `new URL(…,
 * import.meta.url)` is the documented Vite idiom for cross-package
 * asset URLs — it works identically in dev and prod, and Vite emits
 * the hashed asset at build time. We do this lazily inside bootstrap
 * so a dev-mode resolution failure is recoverable (we just fall back
 * to Engine.init's default fetch path instead of crashing the module).
 */
function resolveWasmUrl(): string | null {
  try {
    return new URL(
      "../../../kernel-wasm/vcad_kernel_wasm_bg.wasm",
      import.meta.url,
    ).href;
  } catch {
    return null;
  }
}

/**
 * Module-level guard: React StrictMode double-invokes effects in dev, and
 * we also want a cached promise so any accidental re-entry returns the
 * same in-flight work. Mirrors the `engineInitPromise` pattern that used
 * to live in useEngine.ts.
 */
let bootstrapPromise: Promise<void> | null = null;

/** Timeout for the initial SW update check — offline-friendly. */
const UPDATE_CHECK_TIMEOUT_MS = 1500;
/** How long to show the "Updating..." message before reload — pure polish. */
const UPDATING_DISPLAY_DELAY_MS = 400;
/** Slow-network heuristic: under 100 KB/s after 2s of fetching. */
const SLOW_NETWORK_AFTER_MS = 2000;
const SLOW_NETWORK_THRESHOLD_BYTES_PER_MS = 100;

export function bootstrap(): Promise<void> {
  if (bootstrapPromise) return bootstrapPromise;
  bootstrapPromise = runBootstrap().catch((e) => {
    const msg = e instanceof Error ? e.message : String(e);
    useBootStore.getState().setError(msg);
    useEngineStore.getState().setError(msg);
    throw e;
  });
  return bootstrapPromise;
}

async function runBootstrap(): Promise<void> {
  const boot = useBootStore.getState();

  // ── Phase 1: check for pending service-worker update ──────────────────
  boot.setPhase("checking-update");
  performance.mark("boot-start");

  const updated = await checkForUpdate();
  if (updated) {
    // updateSW(true) reloads the page, so execution stops here on success.
    return;
  }

  // ── Phase 2: fetch + instantiate the kernel WASM ────────────────────
  // The wasm-bindgen instantiate happens inside Engine.init; it's fast
  // enough on a pre-fetched buffer that it's not worth its own phase.
  boot.setPhase("fetching-kernel");
  const wasmBuffer = await fetchKernelWasmWithProgress();
  // Narrow to ArrayBuffer — wasm-bindgen's `BufferSource` rejects
  // `ArrayBufferLike` (which may be a SharedArrayBuffer) but any buffer
  // we built from a fetch stream is always a plain ArrayBuffer.
  const engine = await Engine.init(
    wasmBuffer
      ? { wasmInput: wasmBuffer.buffer as ArrayBuffer }
      : undefined,
  );

  // ── Phase 3: CRDT document engine + AI tool schemas ──────────────────
  useBootStore.getState().setPhase("starting-engine");
  useEngineStore.getState().setEngine(engine);
  await initCrdtAndSchemas();

  // ── Phase 4: GPU + ray tracer (best-effort) ──────────────────────────
  useBootStore.getState().setPhase("loading-gpu");
  try {
    const gpuAvailable = await initializeGpu();
    const raytraceAvailable = gpuAvailable ? await initializeRayTracer() : false;
    useUiStore.getState().setRaytraceAvailable(raytraceAvailable);
  } catch (e) {
    logger.warn("gpu", `Failed to initialize: ${e}`);
  }

  // ── Phase 6: initial document load ───────────────────────────────────
  useBootStore.getState().setPhase("loading-document");
  await initDocument();

  // ── Phase 7: first evaluation ────────────────────────────────────────
  useBootStore.getState().setPhase("evaluating");
  const doc = useDocumentStore.getState().document;
  if (doc.roots.length > 0) {
    try {
      const scene = await engine.evaluateAsync(doc, {
        skipClashDetection: true,
      });
      useEngineStore.getState().setScene(scene);
      scheduleDeferredClash(engine);
    } catch (e) {
      useEngineStore.getState().setError(String(e));
    }
  }

  // ── Phase 8: done ────────────────────────────────────────────────────
  useEngineStore.getState().setEngineReady(true);
  useEngineStore.getState().setLoading(false);
  useBootStore.getState().setPhase("ready");
  performance.mark("boot-complete");
  try {
    performance.measure("boot-total", "boot-start", "boot-complete");
  } catch {
    // performance.measure throws in rare cases — non-fatal
  }
}

/**
 * Race a SW update check against a short timeout. Returns true if an
 * update was consumed (page will reload). Swallows all errors so an
 * offline / misconfigured SW never blocks boot.
 */
async function checkForUpdate(): Promise<boolean> {
  try {
    let updateSW: ((reload?: boolean) => Promise<void>) | null = null;
    const needRefresh = new Promise<boolean>((resolve) => {
      try {
        updateSW = registerSW({
          onNeedRefresh() {
            resolve(true);
          },
          onRegisteredSW(_, r) {
            // Kick off an immediate update check. If nothing pending, the
            // timeout below wins and boot proceeds.
            r?.update().catch(() => {});
          },
          onRegisterError() {
            resolve(false);
          },
        });
      } catch {
        resolve(false);
      }
    });

    const hasUpdate = await Promise.race([
      needRefresh,
      new Promise<boolean>((resolve) =>
        setTimeout(() => resolve(false), UPDATE_CHECK_TIMEOUT_MS),
      ),
    ]);

    if (hasUpdate && updateSW) {
      useBootStore.getState().setPhase("updating");
      await new Promise((r) => setTimeout(r, UPDATING_DISPLAY_DELAY_MS));
      await (updateSW as (reload?: boolean) => Promise<void>)(true);
      // Reload in progress; don't continue bootstrap. Return true to the
      // caller, which simply returns without running downstream phases.
      return true;
    }
  } catch (e) {
    logger.debug("app", `update check failed (non-fatal): ${e}`);
  }
  return false;
}

/**
 * Stream-fetch the kernel WASM, updating the boot store with byte-level
 * progress as chunks arrive. Falls back to `null` on any error, in which
 * case Engine.init() will use its own default fetch path.
 */
async function fetchKernelWasmWithProgress(): Promise<Uint8Array | null> {
  const wasmUrl = resolveWasmUrl();
  if (!wasmUrl) return null;
  try {
    const resp = await fetch(wasmUrl);
    if (!resp.ok || !resp.body) {
      logger.warn("app", `kernel wasm fetch returned ${resp.status}`);
      return null;
    }

    const total = Number(resp.headers.get("content-length") ?? 0);
    useBootStore.getState().setFetchProgress(0, total);

    const reader = resp.body.getReader();
    const chunks: Uint8Array[] = [];
    let received = 0;
    const start = performance.now();
    // eslint-disable-next-line no-constant-condition
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value) continue;
      chunks.push(value);
      received += value.length;
      useBootStore.getState().setFetchProgress(received, total);

      const elapsed = performance.now() - start;
      if (
        !useBootStore.getState().slowNetwork &&
        elapsed > SLOW_NETWORK_AFTER_MS &&
        received / elapsed < SLOW_NETWORK_THRESHOLD_BYTES_PER_MS
      ) {
        useBootStore.getState().setSlowNetwork(true);
      }
    }

    const buffer = new Uint8Array(received);
    let offset = 0;
    for (const chunk of chunks) {
      buffer.set(chunk, offset);
      offset += chunk.length;
    }
    return buffer;
  } catch (e) {
    logger.warn(
      "app",
      `fetch-with-progress failed, falling back to default init: ${e}`,
    );
    return null;
  }
}

/**
 * Import @vcad/kernel-wasm directly (NOT via @vcad/core's getKernelWasm
 * helper) and wire up the CRDT document engine + AI tool schemas. The
 * direct-import constraint exists because the Vite alias only maps a
 * single resolution from the app's import graph — going through core
 * produces a second WASM instance whose pointers don't match.
 */
async function initCrdtAndSchemas(): Promise<void> {
  try {
    const wasmModule = await import("@vcad/kernel-wasm");
    const EngineClass = (wasmModule as Record<string, unknown>)
      .WasmDocumentEngine as (new () => unknown) | undefined;
    if (EngineClass) {
      useDocumentStore.getState()._initCrdt(EngineClass as never);
      logger.info("wasm", "CRDT document engine initialized");
    }
    const getToolSchemas = (wasmModule as Record<string, unknown>)
      .get_tool_schemas as (() => string) | undefined;
    if (getToolSchemas) {
      commandRegistry.loadSchemas(getToolSchemas());
      logger.info(
        "wasm",
        `Loaded ${commandRegistry.getSchemas().length} tool schemas`,
      );
    }
  } catch (e) {
    logger.warn("wasm", `Failed to initialize CRDT engine: ${e}`);
  }
}

/**
 * Boot-time document load: URL (shared link) → most recent IDB doc → new
 * blank document. Lifted verbatim from the useEffect that used to live in
 * App.tsx so there's a single source of truth for initial doc state.
 */
async function initDocument(): Promise<void> {
  try {
    const urlDoc = await loadDocumentFromUrl();
    if (urlDoc) {
      const id = crypto.randomUUID();
      useDocumentStore.getState().loadDocument(urlDoc.file);
      useDocumentStore.getState().setDocumentMeta(id, urlDoc.name);
      useNotificationStore.getState().addToast(
        "Loaded shared document",
        "success",
      );
      return;
    }

    const recent = await getMostRecentDocument();
    if (recent) {
      const stored = await loadDocumentFromDb(recent.id);
      if (stored) {
        useDocumentStore.getState().loadDocument(stored.document);
        useDocumentStore.getState().setDocumentMeta(stored.id, stored.name);
        return;
      }
    }

    const name = await generateDocumentName();
    const id = crypto.randomUUID();
    useDocumentStore.getState().newDocument(id, name);
  } catch (err) {
    logger.warn("app", `Failed to initialize document: ${err}`);
    const id = crypto.randomUUID();
    useDocumentStore.getState().newDocument(id, "Untitled");
  }
}

/**
 * Schedule a clash-detection pass off the critical path once the main
 * scene has rendered. Lifted from useEngine.ts so bootstrap can drive the
 * first evaluation without importing the hook.
 */
function scheduleDeferredClash(engine: Engine): void {
  const run = () => {
    try {
      const doc = useDocumentStore.getState().document;
      if (doc.roots.length === 0) return;
      const scene = engine.evaluate(doc, { skipClashDetection: false });
      useEngineStore.getState().setScene(scene);
    } catch (e) {
      useEngineStore.getState().setError(String(e));
    }
  };

  if (typeof requestIdleCallback === "function") {
    requestIdleCallback(run);
  } else {
    setTimeout(run, 200);
  }
}
