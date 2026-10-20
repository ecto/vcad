/**
 * CRDT parameter value + constructors.
 *
 * `CrdtValue` mirrors the Rust `vcad_crdt::Value` enum; the helpers build the
 * tagged JSON the WASM document engine expects. Consumed by the property
 * panel's scrub inputs and the document store's mutation path.
 */

import type { CrdtValue } from "@vcad/ir";

export type { CrdtValue };

/** Helper to create CrdtValue from a number */
export const f64 = (v: number): CrdtValue => ({ F64: v });
/** Helper to create CrdtValue from a 3D vector */
export const vec3 = (x: number, y: number, z: number): CrdtValue => ({
  Vec3: [x, y, z],
});
/** Helper to create CrdtValue from a boolean */
export const bool = (v: boolean): CrdtValue => ({ Bool: v });
