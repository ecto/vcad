/**
 * Forward kinematics solver — backed by Rust WASM.
 *
 * Thin wrapper around the Rust `solve_forward_kinematics` exposed via WASM.
 * The WASM binding returns a Map<string, Transform3D>.
 */

import type { Document, Transform3D } from "@vcad/ir";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let wasmModule: any = null;

async function loadWasm(): Promise<typeof wasmModule | null> {
  if (wasmModule) return wasmModule;
  try {
    wasmModule = await import("@vcad/kernel-wasm");
    return wasmModule;
  } catch {
    return null;
  }
}

// Eagerly start loading
loadWasm();

/**
 * Solve forward kinematics for an assembly document.
 *
 * Returns a Map from instance ID to world Transform3D.
 * Uses the Rust WASM implementation. If WASM is not yet loaded, returns
 * an empty map (callers should ensure WASM is loaded before physics runs).
 */
export function solveForwardKinematics(
  doc: Document,
): Map<string, Transform3D> {
  if (wasmModule?.solveForwardKinematics) {
    try {
      const result = wasmModule.solveForwardKinematics(
        JSON.stringify(doc),
      ) as Record<string, Transform3D>;
      // Convert plain object to Map
      return new Map(Object.entries(result));
    } catch (e) {
      console.warn("[ENGINE] WASM solveForwardKinematics failed:", e);
      return new Map();
    }
  }
  return new Map();
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
