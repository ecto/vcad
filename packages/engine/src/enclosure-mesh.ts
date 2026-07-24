/**
 * Extract enclosure features (cavity, standoffs, wall openings) from a solid's
 * triangle mesh — thin wrapper around the Rust kernel.
 *
 * This is the bridge from the BRep/CAD side to {@link checkEnclosureFit}: the
 * MCP layer evaluates an enclosure document to a mesh, hands the flat
 * positions/indices here, and gets back the axis-aligned interior void plus the
 * posts and wall cutouts inside it.
 *
 * Inside/outside is tested with the **generalized winding number** (Jacobson et
 * al.), sampled on a coarse 3D grid. GWN is robust to the small holes,
 * coincident faces, and stray internal faces real kernel CSG meshes contain —
 * a plain even-odd ray cast is not. The implementation lives in
 * `crates/vcad-kernel-enclosure` (`src/mesh.rs`); the grid pass is
 * O(cells × triangles), which is why it runs in Rust.
 *
 * Assumes a Z-up, roughly box-shaped, open-top enclosure (the 3D-printed tray:
 * walls + floor + standoffs + side cutouts, lid mounting at the rim).
 */

import { enclosureWasm, type EnclosureFeatures } from "./enclosure-fit.js";

/**
 * Extract the cavity, standoffs, and wall openings from a solid mesh. Returns
 * `cavity: null` when no open-top pocket is found (e.g. a solid block).
 *
 * Requires the kernel WASM singleton to be initialized (`await getKernelWasm()`
 * / `Engine.init()`); throws otherwise.
 */
export function extractEnclosureFeatures(
  positions: ArrayLike<number>,
  indices: ArrayLike<number>,
): EnclosureFeatures {
  // Float64Array.from widens a Float32Array losslessly, so the winding-number
  // sums match what the JS implementation computed on the same buffers.
  const json = enclosureWasm().enclosure_features(
    Float64Array.from(positions as ArrayLike<number>),
    Uint32Array.from(indices as ArrayLike<number>),
  );
  return JSON.parse(json);
}
