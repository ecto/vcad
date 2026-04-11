import { Engine } from "@vcad/engine";
import { useEngineStore } from "./stores/engine-store.js";
import { useDocumentStore } from "./stores/document-store.js";
import type { WasmDocumentEngineConstructor } from "./stores/document-store.js";
import { commandRegistry } from "./commands/index.js";

export interface EngineLifecycleOptions {
  /** Called after the engine is initialized and the first evaluation completes. */
  onReady?: () => void;
  /** Return false to skip evaluation (e.g. empty document). Default: evaluates when roots exist. */
  shouldEvaluate?: (doc: { roots: { root: number }[] }) => boolean;
  /** Skip CRDT engine initialization (useful for tests). */
  skipCrdt?: boolean;
}

/**
 * Initialize the WASM engine, evaluate the current document, and subscribe
 * to future document changes. Shared between web app and CLI.
 */
export async function initEngineLifecycle(
  options?: EngineLifecycleOptions,
): Promise<void> {
  const { setEngineReady, setLoading, setError, setScene } =
    useEngineStore.getState();

  const shouldEvaluate =
    options?.shouldEvaluate ?? ((doc) => doc.roots.length > 0);

  setLoading(true);

  try {
    const engine = await Engine.init();
    setEngineReady(true);
    setLoading(false);

    // Initialize CRDT document engine (best-effort, non-blocking)
    if (!options?.skipCrdt) {
      try {
        const wasmModule = await import("@vcad/kernel-wasm");
        const EngineClass = (wasmModule as Record<string, unknown>)
          .WasmDocumentEngine as WasmDocumentEngineConstructor | undefined;
        if (EngineClass) {
          useDocumentStore.getState()._initCrdt(EngineClass);
        }
        // Load tool schemas from WASM into the AI command registry
        const getToolSchemas = (wasmModule as Record<string, unknown>)
          .get_tool_schemas as (() => string) | undefined;
        if (getToolSchemas) {
          commandRegistry.loadSchemas(getToolSchemas());
        }
      } catch (e) {
        // CRDT engine is optional — log and continue
        console.warn("[CRDT] Failed to initialize document engine:", e);
      }
    }

    // Evaluate initial document
    const doc = useDocumentStore.getState().document;
    if (shouldEvaluate(doc)) {
      try {
        setScene(engine.evaluate(doc));
      } catch (e) {
        setError(String(e));
      }
    }

    // Subscribe to document changes and re-evaluate
    useDocumentStore.subscribe((state) => {
      try {
        const scene = engine.evaluate(state.document);
        setScene(scene);
      } catch (e) {
        setError(String(e));
      }
    });

    options?.onReady?.();
  } catch (e) {
    setError(String(e));
    setLoading(false);
  }
}
