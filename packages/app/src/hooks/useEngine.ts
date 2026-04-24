import { useEffect, useRef } from "react";
import {
  useDocumentStore,
  useEngineStore,
  useParametersStore,
  useSimulationStore,
  mergeParametersIntoDocument,
} from "@vcad/core";
import type { Document } from "@vcad/ir";

/** Snapshot parameters + bindings and merge them onto `doc` so the engine
 * sees a parametric document. Cheap — one object spread when both sidecars
 * are empty. */
function docWithParameters(doc: Document): Document {
  const { parameters, bindings } = useParametersStore.getState();
  return mergeParametersIntoDocument(doc, parameters, bindings);
}

/** Debounce timeout for full-quality re-render after drag ends */
let refinementTimeout: ReturnType<typeof setTimeout> | null = null;

/** Monotonically increasing eval request ID for cancellation of stale results */
let evalGeneration = 0;

/**
 * Subscribe to document-store changes and re-evaluate the scene when the
 * document content changes. The boot path (initial engine load, first
 * document load, first evaluation) lives in `lib/bootstrap.ts` — this
 * hook is purely for post-boot reactivity.
 */
export function useEngine() {
  const rafRef = useRef<number>(0);

  useEffect(() => {
    const unsub = useDocumentStore.subscribe((state, prevState) => {
      const engine = useEngineStore.getState().engine;
      if (!engine) return;

      const docChanged = state.document !== prevState?.document;
      const transientEnded =
        !!prevState?.isTransientEval && !state.isTransientEval;
      const dragEnded =
        !!prevState?.isParameterDragging && !state.isParameterDragging;

      // Handle "transient batch just ended without another doc change" by
      // scheduling a refinement pass with full clash detection. This is
      // how the viewport's clash highlights catch up after the AI stops
      // calling tools, or after the user releases a drag whose final
      // position equals the last LOD eval.
      if (!docChanged && (transientEnded || dragEnded)) {
        if (refinementTimeout) {
          clearTimeout(refinementTimeout);
        }
        refinementTimeout = setTimeout(() => {
          const refGen = ++evalGeneration;
          const doc = useDocumentStore.getState().document;
          engine
            .evaluateAsync(docWithParameters(doc), { skipClashDetection: false })
            .then((refinedScene) => {
              if (refGen !== evalGeneration) return;
              useEngineStore.getState().setScene(refinedScene);
            })
            .catch((e) => {
              if (refGen !== evalGeneration) return;
              useEngineStore.getState().setError(String(e));
            });
          refinementTimeout = null;
        }, 100);
        return;
      }

      // Only re-evaluate if the actual document content changed
      // Skip metadata-only changes (isDirty, lastSavedAt, etc.)
      if (!docChanged) {
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

        // Skip clash detection for transient updates (user drag, AI tool
        // call batches). Clash is O(n²) pairwise boolean intersections and
        // dominates large-scene eval cost (~9s on a 53-part bike). Painting
        // the viewport within one animation frame is worth deferring the
        // overlap highlights until the batch quiesces.
        const isTransient =
          state.isParameterDragging || state.isTransientEval;
        const wasTransient =
          (prevState?.isParameterDragging ?? false) ||
          (prevState?.isTransientEval ?? false);

        const dirtyNodes = state.clearDirtyNodes();
        if (dirtyNodes.size > 0) {
          engine.invalidateNodes(dirtyNodes);
        }

        const gen = ++evalGeneration;
        engine
          .evaluateAsync(docWithParameters(state.document), { skipClashDetection: isTransient })
          .then((scene) => {
            if (gen !== evalGeneration) return;
            useEngineStore.getState().setScene(scene);
          })
          .catch((e) => {
            if (gen !== evalGeneration) return;
            useEngineStore.getState().setError(String(e));
          });

        // If the transient batch just ended, schedule a refinement pass with
        // full clash detection.
        if (wasTransient && !isTransient) {
          if (refinementTimeout) {
            clearTimeout(refinementTimeout);
          }
          refinementTimeout = setTimeout(() => {
            const refGen = ++evalGeneration;
            const doc = useDocumentStore.getState().document;
            engine
              .evaluateAsync(docWithParameters(doc), { skipClashDetection: false })
              .then((refinedScene) => {
                if (refGen !== evalGeneration) return;
                useEngineStore.getState().setScene(refinedScene);
              })
              .catch((e) => {
                if (refGen !== evalGeneration) return;
                useEngineStore.getState().setError(String(e));
              });
            refinementTimeout = null;
          }, 100);
        }
      });
    });

    // Re-evaluate when the user edits a parameter value or a binding.
    // Reuse the same RAF-debounced path as document changes.
    const unsubParams = useParametersStore.subscribe((next, prev) => {
      if (next.parameters === prev.parameters && next.bindings === prev.bindings) return;
      const engine = useEngineStore.getState().engine;
      if (!engine) return;
      const simMode = useSimulationStore.getState().mode;
      if (simMode !== "off") return;
      cancelAnimationFrame(rafRef.current);
      rafRef.current = requestAnimationFrame(() => {
        const doc = useDocumentStore.getState().document;
        const gen = ++evalGeneration;
        engine
          .evaluateAsync(docWithParameters(doc), { skipClashDetection: true })
          .then((scene) => {
            if (gen !== evalGeneration) return;
            useEngineStore.getState().setScene(scene);
          })
          .catch((e) => {
            if (gen !== evalGeneration) return;
            useEngineStore.getState().setError(String(e));
          });
      });
    });

    return () => {
      unsub();
      unsubParams();
      cancelAnimationFrame(rafRef.current);
      if (refinementTimeout) {
        clearTimeout(refinementTimeout);
        refinementTimeout = null;
      }
    };
  }, []);
}
