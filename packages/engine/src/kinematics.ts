/**
 * Forward kinematics solver — backed by Rust WASM.
 *
 * Thin wrapper around the Rust `solve_forward_kinematics` exposed via WASM.
 * `serde_wasm_bindgen` serializes the Rust HashMap as a JS Map (not a plain
 * object), so the result is normalized here for both shapes.
 */

import type { Document, Transform3D } from "@vcad/ir";
import { getKernelWasmSync } from "./wasm-singleton.js";

/**
 * Solve forward kinematics for an assembly document.
 *
 * Returns a Map from instance ID to world Transform3D.
 * Uses the Rust WASM implementation via the shared singleton. If the
 * singleton hasn't finished initializing (`getKernelWasm()` not yet
 * awaited anywhere), returns an empty map — callers must ensure WASM is
 * initialized before relying on FK results.
 */
export function solveForwardKinematics(
  doc: Document,
): Map<string, Transform3D> {
  const wasm = getKernelWasmSync();
  if (!wasm?.solveForwardKinematics) return new Map();
  try {
    const result: unknown = wasm.solveForwardKinematics(JSON.stringify(doc));
    // serde_wasm_bindgen returns a JS Map; older glue returned a plain object.
    if (result instanceof Map) {
      return result as Map<string, Transform3D>;
    }
    return new Map(
      Object.entries(result as Record<string, Transform3D>),
    );
  } catch (e) {
    console.warn("[ENGINE] WASM solveForwardKinematics failed:", e);
    return new Map();
  }
}

/**
 * Apply forward kinematics to update instance transforms in place.
 */
export function applyForwardKinematics(doc: Document): void {
  const worldTransforms = solveForwardKinematics(doc);

  if (!doc.instances) return;

  for (const instance of doc.instances) {
    const worldTransform = worldTransforms.get(instance.id);
    if (worldTransform) {
      instance.transform = worldTransform;
    }
  }
}
