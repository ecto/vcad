/**
 * "Fit board to enclosure" geometry.
 *
 * Sizes + positions a PCB board so its *world* footprint fills a target box
 * (the surrounding mechanical bounds) inset by a clearance margin. The board
 * outline lives in board-local space, while the enclosure bounds are world, so
 * this works through the board's full transform (translation, scale, and
 * in-plane rotation): the clearance-inset enclosure box is inverse-mapped into
 * the board's local frame to derive the outline size, and the outline is then
 * placed so its forward-mapped world bbox lands on the enclosure.
 *
 * For board rotations that are multiples of 90° the fit is exact (W/H swap as
 * needed); for arbitrary in-plane angles it fits the board's bounding box to
 * the enclosure (a slight diagonal overflow), which is the honest best effort.
 * Out-of-plane tilt is treated as a footprint projection.
 */

import * as THREE from "three";
import type { PcbBoardTransform } from "@vcad/core";
import type { Aabb } from "./pcb-interference";

export interface BoardFit {
  /** Board-local outline width (mm), pre-scale. */
  width: number;
  /** Board-local outline height (mm), pre-scale. */
  height: number;
  /** World translation for the board's Translate node (Z preserved). */
  position: { x: number; y: number; z: number };
}

/**
 * Compute the outline size + world position that fits a board to `enc` (the
 * surrounding mechanical world AABB) inset by `clearance` on every XY side.
 * Returns null when the inset target is degenerate (< 1mm on a side).
 */
export function computeBoardFit(
  enc: Aabb,
  xf: PcbBoardTransform,
  clearance: number,
): BoardFit | null {
  const minX = enc.min[0] + clearance;
  const minY = enc.min[1] + clearance;
  const maxX = enc.max[0] - clearance;
  const maxY = enc.max[1] - clearance;
  if (maxX - minX < 1 || maxY - minY < 1) return null;
  const midZ = (enc.min[2] + enc.max[2]) / 2;

  // Rotation+scale only (no translation): maps board-local → world directions.
  const euler = new THREE.Euler(
    (xf.rotationDeg.x * Math.PI) / 180,
    (xf.rotationDeg.y * Math.PI) / 180,
    (xf.rotationDeg.z * Math.PI) / 180,
    "XYZ",
  );
  const rs = new THREE.Matrix4().compose(
    new THREE.Vector3(),
    new THREE.Quaternion().setFromEuler(euler),
    new THREE.Vector3(xf.scale.x || 1, xf.scale.y || 1, xf.scale.z || 1),
  );
  const rsInv = rs.clone().invert();

  // Inverse-map the inset enclosure XY corners into the board's local frame;
  // their local bbox is the outline size (scale already divided out).
  const v = new THREE.Vector3();
  let lminX = Infinity, lminY = Infinity, lmaxX = -Infinity, lmaxY = -Infinity;
  for (const [x, y] of [[minX, minY], [maxX, minY], [maxX, maxY], [minX, maxY]]) {
    v.set(x!, y!, midZ).applyMatrix4(rsInv);
    if (v.x < lminX) lminX = v.x;
    if (v.x > lmaxX) lmaxX = v.x;
    if (v.y < lminY) lminY = v.y;
    if (v.y > lmaxY) lmaxY = v.y;
  }
  const width = lmaxX - lminX;
  const height = lmaxY - lminY;

  // Forward-map the origin-cornered outline (0,0)-(w,h) through rotation+scale
  // (no translation) to find its world-min, then translate so that min lands on
  // the enclosure's inset min corner.
  let oMinX = Infinity, oMinY = Infinity;
  for (const [x, y] of [[0, 0], [width, 0], [width, height], [0, height]]) {
    v.set(x!, y!, 0).applyMatrix4(rs);
    if (v.x < oMinX) oMinX = v.x;
    if (v.y < oMinY) oMinY = v.y;
  }

  return {
    width,
    height,
    position: { x: minX - oMinX, y: minY - oMinY, z: xf.position.z },
  };
}
