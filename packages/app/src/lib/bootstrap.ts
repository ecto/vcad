import {
  Engine,
  useDocumentStore,
  useEngineStore,
  useUiStore,
  logger,
  commandRegistry,
  primeKernelWasm,
  type VcadFile,
} from "@vcad/core";
import { initializeGpu, initializeRayTracer } from "@vcad/engine";
import { registerSW } from "virtual:pwa-register";

import { useBootStore } from "@/stores/boot-store";
import {
  getMostRecentDocument,
  loadDocument as loadDocumentFromDb,
  generateDocumentName,
} from "@/lib/storage";
import { loadDocumentFromUrl, getLocalDocRouteId } from "@/lib/url-document";
import type { UrlDocumentResult } from "@/lib/url-document";
import { newDocId } from "@/lib/doc-id";
import { useNotificationStore } from "@/stores/notification-store";
import { analytics } from "@/lib/analytics";

/**
 * One-shot reload when a dynamically imported chunk's hashed URL no longer
 * exists — the classic "deployed a new version while the tab was open" case.
 * Retrying is useless (the file is gone), but a reload fetches the fresh
 * entry point and new chunk manifest.
 */
const PRELOAD_RELOAD_KEY = "vcad_reloaded_for_preload";
if (typeof window !== "undefined") {
  window.addEventListener("vite:preloadError", (event) => {
    if (sessionStorage.getItem(PRELOAD_RELOAD_KEY)) return;
    event.preventDefault();
    sessionStorage.setItem(PRELOAD_RELOAD_KEY, "1");
    analytics.chunkLoadStaleDeploy();
    window.location.reload();
  });
}

/** Slow-network heuristic: under 100 KB/s after 2s of fetching. */
const SLOW_NETWORK_AFTER_MS = 2000;
const SLOW_NETWORK_THRESHOLD_BYTES_PER_MS = 100;

/**
 * Cached promise so React StrictMode's double-effect doesn't re-enter boot.
 */
let bootstrapPromise: Promise<void> | null = null;

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
  performance.mark("boot-start");

  // SW update check runs on the side — a waiting update surfaces as a
  // "Reload" toast instead of blocking the boot.
  startBackgroundUpdateCheck();

  // ── Critical path: fetch kernel + hydrate document data in parallel ──
  useBootStore.getState().setPhase("fetching-kernel");
  const wasmReady = fetchAndPrimeKernelWasm().then(() => Engine.init());
  const docDataReady = fetchDocumentData();

  const engine = await wasmReady;
  useEngineStore.getState().setEngine(engine);

  useBootStore.getState().setPhase("starting-engine");
  const wasmModule = await import("@vcad/kernel-wasm");
  initCrdt(wasmModule);

  useBootStore.getState().setPhase("loading-document");
  applyDocumentData(await docDataReady);

  useBootStore.getState().setPhase("evaluating");
  await evaluateInitialScene(engine);

  useEngineStore.getState().setEngineReady(true);
  useEngineStore.getState().setLoading(false);
  useBootStore.getState().setPhase("ready");

  performance.mark("boot-complete");
  try {
    performance.measure("boot-total", "boot-start", "boot-complete");
  } catch {
    // performance.measure throws in rare cases — non-fatal
  }

  // Anything the user can't see or use on the first frame goes here.
  scheduleIdle(() => {
    void initGpuStack();
    loadToolSchemas(wasmModule);
    scheduleDeferredClash(engine);
    void runIdbBackfillDeferred(wasmModule);
  });
}

/**
 * Migrate any legacy-format IDB rows to the canonical CRDT v0.4 shape.
 * Idempotent — already-migrated rows are skipped. Runs after the main
 * boot finishes so the user's first frame isn't delayed by it.
 */
async function runIdbBackfillDeferred(
  wasmModule: typeof import("@vcad/kernel-wasm"),
): Promise<void> {
  try {
    const [{ runIdbBackfill }, { triggerSync }] = await Promise.all([
      import("@/lib/backfill"),
      import("@vcad/auth"),
    ]);
    await runIdbBackfill(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      wasmModule.WasmDocumentEngine as any,
      () => triggerSync(),
    );
  } catch (e) {
    logger.warn("app", `IDB backfill failed: ${e}`);
  }
}

/**
 * Resolve the kernel WASM URL through Vite's asset pipeline. `new URL(…,
 * import.meta.url)` is the documented Vite idiom for cross-package asset
 * URLs — it works identically in dev and prod, and Vite emits the hashed
 * asset at build time.
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
 * Stream-fetch the kernel WASM and hand it to the singleton as a Response
 * so wasm-bindgen can `instantiateStreaming` — compile overlaps with
 * download. A tee'd copy of the body stream drives the splash progress
 * bar without slowing the compile path.
 */
async function fetchAndPrimeKernelWasm(): Promise<void> {
  const wasmUrl = resolveWasmUrl();
  if (!wasmUrl) return;

  let resp: Response;
  try {
    resp = await fetch(wasmUrl);
  } catch (e) {
    logger.warn("app", `kernel wasm fetch failed: ${e}`);
    return;
  }
  if (!resp.ok || !resp.body) {
    logger.warn("app", `kernel wasm fetch returned ${resp.status}`);
    return;
  }

  const total = Number(resp.headers.get("content-length") ?? 0);
  useBootStore.getState().setFetchProgress(0, total);

  const [compileStream, progressStream] = resp.body.tee();
  const compileResponse = new Response(compileStream, {
    headers: { "content-type": "application/wasm" },
  });
  primeKernelWasm(compileResponse);

  // Fire-and-forget: Engine.init() will await the streaming compile.
  void trackDownloadProgress(progressStream, total);
}

async function trackDownloadProgress(
  stream: ReadableStream<Uint8Array>,
  total: number,
): Promise<void> {
  const reader = stream.getReader();
  let received = 0;
  const start = performance.now();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value) continue;
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
  } catch (e) {
    logger.debug("app", `progress stream ended early: ${e}`);
  }
}

/**
 * Register the service worker and, if an update is waiting, surface it as
 * a dismissible "Reload" toast. All errors are swallowed — an offline /
 * misconfigured SW must never block or disturb boot.
 */
function startBackgroundUpdateCheck(): void {
  if (typeof window === "undefined") return;
  try {
    let updateSW: ((reload?: boolean) => Promise<void>) | null = null;
    updateSW = registerSW({
      onNeedRefresh() {
        useNotificationStore.getState().showActionResult({
          type: "success",
          title: "New version available",
          description: "Reload to apply the latest vcad build.",
          actions: [
            {
              label: "Reload",
              variant: "primary",
              onClick: () => {
                void updateSW?.(true);
              },
            },
          ],
        });
      },
      onRegisteredSW(_, r) {
        // Kick off an immediate update check; the onNeedRefresh callback
        // fires if something is actually waiting.
        r?.update().catch(() => {});
      },
      onRegisterError() {},
    });
  } catch (e) {
    logger.debug("app", `SW registration failed (non-fatal): ${e}`);
  }
}

/**
 * Resolved at boot and applied after the CRDT engine is wired up. Parking
 * the data in a plain object lets us run the IDB / URL lookup concurrent
 * with the kernel fetch — the actual store mutations happen serially
 * once everything the store needs is ready.
 */
type DocumentBootData =
  | { kind: "url"; urlDoc: UrlDocumentResult }
  | { kind: "stored"; id: string; name: string; file: VcadFile }
  | { kind: "new"; id: string; name: string };

async function fetchDocumentData(): Promise<DocumentBootData> {
  try {
    const urlDoc = await loadDocumentFromUrl();
    if (urlDoc) return { kind: "url", urlDoc };

    // `/~<localId>` — URL carries the exact local doc to open. If the doc
    // doesn't exist on this device, fall through to the most-recent flow
    // rather than crash; the URL is replaced by the mirror after loading.
    const routedId = getLocalDocRouteId();
    if (routedId) {
      const stored = await loadDocumentFromDb(routedId);
      if (stored) {
        return {
          kind: "stored",
          id: stored.id,
          name: stored.name,
          file: stored.document,
        };
      }
      logger.warn("app", `URL referenced unknown local doc ${routedId}, falling back`);
    }

    const recent = await getMostRecentDocument();
    if (recent) {
      const stored = await loadDocumentFromDb(recent.id);
      if (stored) {
        return {
          kind: "stored",
          id: stored.id,
          name: stored.name,
          file: stored.document,
        };
      }
    }

    const name = await generateDocumentName();
    return { kind: "new", id: newDocId(), name };
  } catch (err) {
    logger.warn("app", `Failed to resolve initial document: ${err}`);
    return { kind: "new", id: newDocId(), name: "Untitled" };
  }
}

function applyDocumentData(data: DocumentBootData): void {
  const docStore = useDocumentStore.getState();

  if (data.kind === "stored") {
    docStore.loadDocument(data.file);
    docStore.setDocumentMeta(data.id, data.name);
    return;
  }

  if (data.kind === "new") {
    docStore.newDocument(data.id, data.name);
    return;
  }

  // kind === "url"
  const { urlDoc } = data;
  const id = newDocId();
  docStore.loadDocument(urlDoc.file);
  docStore.setDocumentMeta(id, urlDoc.name);

  if (urlDoc.readOnlyShareToken) {
    useUiStore.getState().setReadOnlyShare({
      token: urlDoc.readOnlyShareToken,
      docName: urlDoc.name,
    });
    useNotificationStore
      .getState()
      .toast.info(`Viewing ${urlDoc.name} (read-only)`);

    if (urlDoc.viewerStateHint) {
      // Apply viewer state on the next frame so the doc has rendered once
      // before we move the camera and restore selection.
      const hint = urlDoc.viewerStateHint;
      setTimeout(() => {
        void import("@/lib/viewer-state").then(({ applyViewerStateHint }) => {
          applyViewerStateHint(hint);
        });
      }, 100);
    }
  } else {
    useNotificationStore.getState().addToast("Loaded shared document", "success");
  }
}

/**
 * Import @vcad/kernel-wasm directly (NOT via @vcad/core's getKernelWasm
 * helper) and wire up the CRDT document engine. The direct-import
 * constraint exists because the Vite alias only maps a single resolution
 * from the app's import graph — going through core produces a second
 * WASM instance whose pointers don't match.
 */
function initCrdt(
  wasmModule: typeof import("@vcad/kernel-wasm"),
): void {
  try {
    const EngineClass = (wasmModule as Record<string, unknown>)
      .WasmDocumentEngine as (new () => unknown) | undefined;
    if (EngineClass) {
      useDocumentStore.getState()._initCrdt(EngineClass as never);
      logger.info("wasm", "CRDT document engine initialized");
    }
  } catch (e) {
    logger.warn("wasm", `Failed to initialize CRDT engine: ${e}`);
  }
}

/**
 * Load AI tool schemas from the kernel WASM. Deferred off the critical
 * path — chat ships with baked-in `STATIC_TOOL_SCHEMAS` fallbacks, so
 * the registry is usable from the moment the app renders.
 */
function loadToolSchemas(
  wasmModule: typeof import("@vcad/kernel-wasm"),
): void {
  try {
    const getToolSchemas = (wasmModule as Record<string, unknown>)
      .get_tool_schemas as (() => string) | undefined;
    if (!getToolSchemas) return;
    commandRegistry.loadSchemas(getToolSchemas());
    logger.info(
      "wasm",
      `Loaded ${commandRegistry.getSchemas().length} tool schemas`,
    );
  } catch (e) {
    logger.warn("wasm", `Failed to load tool schemas: ${e}`);
  }
}

/** Best-effort GPU + ray-tracer probe. Not required for the first frame. */
async function initGpuStack(): Promise<void> {
  try {
    const gpuAvailable = await initializeGpu();
    const raytraceAvailable = gpuAvailable
      ? await initializeRayTracer()
      : false;
    useUiStore.getState().setRaytraceAvailable(raytraceAvailable);
  } catch (e) {
    logger.warn("gpu", `Failed to initialize: ${e}`);
  }
}

async function evaluateInitialScene(engine: Engine): Promise<void> {
  const doc = useDocumentStore.getState().document;
  if (doc.roots.length === 0) return;
  try {
    const scene = await engine.evaluateAsync(doc, {
      skipClashDetection: true,
    });
    useEngineStore.getState().setScene(scene);
  } catch (e) {
    useEngineStore.getState().setError(String(e));
  }
}

/**
 * Schedule a clash-detection pass off the critical path once the main
 * scene has rendered.
 */
function scheduleDeferredClash(engine: Engine): void {
  const doc = useDocumentStore.getState().document;
  if (doc.roots.length === 0) return;
  try {
    const scene = engine.evaluate(doc, { skipClashDetection: false });
    useEngineStore.getState().setScene(scene);
  } catch (e) {
    useEngineStore.getState().setError(String(e));
  }
}

function scheduleIdle(fn: () => void): void {
  if (
    typeof window !== "undefined" &&
    typeof window.requestIdleCallback === "function"
  ) {
    window.requestIdleCallback(fn);
  } else {
    setTimeout(fn, 200);
  }
}
