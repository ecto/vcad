import { useEffect, useRef } from "react";
import { Engine, useDocumentStore, useEngineStore, useSimulationStore, useUiStore, logger, commandRegistry } from "@vcad/core";
import { initializeGpu, initializeRayTracer } from "@vcad/engine";
import type { Document } from "@vcad/ir";

// Module-level engine instance to survive HMR
let globalEngine: Engine | null = null;
// Guard against concurrent init (React StrictMode calls effect twice)
let engineInitPromise: Promise<Engine> | null = null;

/** Debounce timeout for full-quality re-render after drag ends */
let refinementTimeout: ReturnType<typeof setTimeout> | null = null;

/** Monotonically increasing eval request ID for cancellation of stale results */
let evalGeneration = 0;

/** Schedule deferred clash detection via requestIdleCallback (or setTimeout fallback). */
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

export function useEngine() {
  const engineRef = useRef<Engine | null>(globalEngine);
  const rafRef = useRef<number>(0);

  // Init engine (only once, survives HMR)
  useEffect(() => {
    // If engine already exists (from previous HMR cycle), reuse it
    if (globalEngine) {
      engineRef.current = globalEngine;
      useEngineStore.getState().setEngine(globalEngine);
      useEngineStore.getState().setEngineReady(true);
      useEngineStore.getState().setLoading(false);

      // Re-evaluate the document to restore the scene (async, off main thread)
      const doc = useDocumentStore.getState().document;
      if (doc.roots.length > 0) {
        const gen = ++evalGeneration;
        globalEngine.evaluateAsync(doc, { skipClashDetection: true }).then((scene) => {
          if (gen !== evalGeneration) return; // stale
          useEngineStore.getState().setScene(scene);
          scheduleDeferredClash(globalEngine!);
        }).catch((e) => {
          useEngineStore.getState().setError(String(e));
        });
      }
      return;
    }

    let cancelled = false;
    let initDocSub: (() => void) | null = null;
    useEngineStore.getState().setLoading(true);
    performance.mark("engine-init-start");

    // If init is already in progress (React StrictMode), reuse the existing promise
    if (!engineInitPromise) {
      engineInitPromise = Engine.init();
    }

    engineInitPromise
      .then(async (engine) => {
        if (cancelled) return;
        performance.mark("engine-init-complete");
        globalEngine = engine;
        engineRef.current = engine;
        useEngineStore.getState().setEngine(engine);
        useEngineStore.getState().setEngineReady(true);
        useEngineStore.getState().setLoading(false);

        // Initialize CRDT document engine (best-effort, non-blocking)
        // IMPORTANT: use import() directly here — NOT via @vcad/core's getKernelWasm().
        // The Vite alias maps @vcad/kernel-wasm to a single file, but only for imports
        // originating from the app. Importing via core creates a separate WASM instance
        // which invalidates pointers from the first instance.
        logger.debug("wasm", "useEngine: starting CRDT init via import(@vcad/kernel-wasm)");
        import("@vcad/kernel-wasm")
          .then((wasmModule) => {
            logger.debug("wasm", "useEngine: CRDT import resolved");
            const EngineClass = (wasmModule as Record<string, unknown>)
              .WasmDocumentEngine as (new () => unknown) | undefined;
            if (EngineClass) {
              useDocumentStore.getState()._initCrdt(EngineClass as never);
              logger.info("wasm", "CRDT document engine initialized");
            }
            // Load tool schemas into the AI command registry
            const getToolSchemas = (wasmModule as Record<string, unknown>)
              .get_tool_schemas as (() => string) | undefined;
            if (getToolSchemas) {
              commandRegistry.loadSchemas(getToolSchemas());
              logger.info("wasm", `Loaded ${commandRegistry.getSchemas().length} tool schemas`);
            }
          })
          .catch((e) => {
            logger.warn("wasm", `Failed to initialize CRDT engine: ${e}`);
          });

        // Initialize GPU for accelerated geometry processing (non-blocking)
        initializeGpu()
          .then((gpuAvailable) => {
            // After GPU init, try to initialize ray tracer
            if (gpuAvailable) {
              return initializeRayTracer();
            }
            return false;
          })
          .then((raytraceAvailable) => {
            useUiStore.getState().setRaytraceAvailable(raytraceAvailable);
          })
          .catch((e) => {
            logger.warn("gpu", `Failed to initialize: ${e}`);
          });

        // Evaluate initial document — skip clashes for fast first paint.
        // The document may not have loaded from IDB yet (race with App initDocument),
        // so if roots are empty, subscribe for the first non-empty document.
        const evalInitialDoc = (doc: Document) => {
          const gen = ++evalGeneration;
          engine.evaluateAsync(doc, { skipClashDetection: true }).then((scene) => {
            if (gen !== evalGeneration) return; // stale
            useEngineStore.getState().setScene(scene);
            scheduleDeferredClash(engine);
          }).catch((e) => {
            useEngineStore.getState().setError(String(e));
          });
        };

        const doc = useDocumentStore.getState().document;
        if (doc.roots.length > 0) {
          evalInitialDoc(doc);
        } else {
          // Watch for document load from IDB
          initDocSub = useDocumentStore.subscribe((state) => {
            if (state.document.roots.length > 0) {
              initDocSub?.();
              initDocSub = null;
              evalInitialDoc(state.document);
            }
          });
        }
      })
      .catch((e) => {
        if (!cancelled) useEngineStore.getState().setError(String(e));
      });

    return () => {
      cancelled = true;
      initDocSub?.();
      initDocSub = null;
    };
  }, []); // Empty deps - only run on initial mount

  // Subscribe to document changes and re-evaluate
  useEffect(() => {
    const unsub = useDocumentStore.subscribe((state, prevState) => {
      // Use globalEngine for HMR stability (engineRef might be stale)
      const engine = globalEngine;
      if (!engine) return;

      // Only re-evaluate if the actual document content changed
      // Skip metadata-only changes (isDirty, lastSavedAt, etc.)
      if (state.document === prevState?.document) {
        return;
      }

      // Check mode BEFORE scheduling RAF - if physics is active, skip entirely
      const simModeNow = useSimulationStore.getState().mode;
      if (simModeNow !== "off") {
        return;
      }

      // Debounce to next animation frame
      cancelAnimationFrame(rafRef.current);
      rafRef.current = requestAnimationFrame(() => {
        // Double-check mode inside RAF (mode might have changed since scheduling)
        const simMode = useSimulationStore.getState().mode;
        if (simMode !== "off") {
          return;
        }

        // Skip re-evaluation for empty documents if we already have a scene
        // (preserves imported STL/STEP meshes that bypass the document model)
        if (state.document.roots.length === 0) {
          const currentScene = useEngineStore.getState().scene;
          const wasAlreadyEmpty = prevState?.document.roots.length === 0;
          if (currentScene && currentScene.parts.length > 0 && wasAlreadyEmpty) {
            return;
          }
        }

        // During parameter dragging: skip clash detection for faster updates
        const isDragging = state.isParameterDragging;

        // Get dirty nodes and clear them
        const dirtyNodes = state.clearDirtyNodes();

        // Invalidate caches for dirty nodes (if any)
        if (dirtyNodes.size > 0) {
          engine.invalidateNodes(dirtyNodes);
        }

        // Async evaluation — off main thread when worker is available
        const gen = ++evalGeneration;
        engine.evaluateAsync(state.document, {
          skipClashDetection: isDragging,
        }).then((scene) => {
          if (gen !== evalGeneration) return; // stale — newer eval superseded this one
          useEngineStore.getState().setScene(scene);
        }).catch((e) => {
          if (gen !== evalGeneration) return;
          useEngineStore.getState().setError(String(e));
        });

        // If dragging just ended, schedule a refinement pass with clash detection
        if (prevState?.isParameterDragging && !isDragging) {
          if (refinementTimeout) {
            clearTimeout(refinementTimeout);
          }
          refinementTimeout = setTimeout(() => {
            const refGen = ++evalGeneration;
            const doc = useDocumentStore.getState().document;
            engine.evaluateAsync(doc, { skipClashDetection: false }).then((refinedScene) => {
              if (refGen !== evalGeneration) return;
              useEngineStore.getState().setScene(refinedScene);
            }).catch((e) => {
              if (refGen !== evalGeneration) return;
              useEngineStore.getState().setError(String(e));
            });
            refinementTimeout = null;
          }, 100);
        }
      });
    });

    return () => {
      unsub();
      cancelAnimationFrame(rafRef.current);
      if (refinementTimeout) {
        clearTimeout(refinementTimeout);
        refinementTimeout = null;
      }
    };
  }, []); // Empty deps - subscription is stable
}
