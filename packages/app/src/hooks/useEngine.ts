import { useEffect, useRef } from "react";
import {
  useDocumentStore,
  useEngineStore,
  useSimulationStore,
} from "@vcad/core";

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

        const dirtyNodes = state.clearDirtyNodes();
        if (dirtyNodes.size > 0) {
          engine.invalidateNodes(dirtyNodes);
        }

        const gen = ++evalGeneration;
        engine
          .evaluateAsync(state.document, { skipClashDetection: isDragging })
          .then((scene) => {
            if (gen !== evalGeneration) return;
            useEngineStore.getState().setScene(scene);
          })
          .catch((e) => {
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
            engine
              .evaluateAsync(doc, { skipClashDetection: false })
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

    return () => {
      unsub();
      cancelAnimationFrame(rafRef.current);
      if (refinementTimeout) {
        clearTimeout(refinementTimeout);
        refinementTimeout = null;
      }
    };
  }, []);
}
