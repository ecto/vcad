import { useEffect, useRef } from "react";
import * as THREE from "three";
import {
  useCoreElectronicsStore,
  useDocumentStore,
  getNodePcb,
  findPcbBoardPart,
  getPcbBoardTransform,
} from "@vcad/core";

/**
 * Auto-fit the viewport camera to the focused PCB board the first time a
 * board gains edit focus. Fires `vcad:face-selected` (kernel Z-up) — the
 * handler in ViewportContent swings the camera perpendicular to the board
 * and then re-enables OrbitControls, so this is a one-off framing, not a
 * camera lock. Skips when a share URL supplied a `?at=` viewer-state hint,
 * since that hint owns the camera.
 *
 * Mirrors `useSketchAutoFit`: fires once per focus session. Leaving and
 * re-entering a board re-frames it; editing within a session does not yank
 * the camera away from wherever the user orbited to.
 */
export function usePcbAutoFit(): void {
  const boardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const firedRef = useRef(false);

  // Reset the one-shot guard when edit focus clears.
  useEffect(() => {
    if (boardNodeId == null) firedRef.current = false;
  }, [boardNodeId]);

  useEffect(() => {
    if (boardNodeId == null || firedRef.current) return;

    if (typeof window !== "undefined") {
      const params = new URLSearchParams(window.location.search);
      if (params.has("at")) return;
    }

    const doc = useDocumentStore.getState().document;
    const pcb = getNodePcb(doc, boardNodeId);
    const verts = pcb?.outline?.vertices;
    if (!pcb || !verts || verts.length < 3) return;

    // The board may have been moved/rotated as a part — frame it where the
    // kernel slab actually renders, not at the origin. Compose the board's
    // world transform (identity for an unmoved board).
    const part = findPcbBoardPart(useDocumentStore.getState().parts, boardNodeId);
    const xf = part ? getPcbBoardTransform(doc, part) : null;
    const mat = new THREE.Matrix4();
    if (xf) {
      mat.compose(
        new THREE.Vector3(xf.position.x, xf.position.y, xf.position.z),
        new THREE.Quaternion().setFromEuler(
          new THREE.Euler(
            (xf.rotationDeg.x * Math.PI) / 180,
            (xf.rotationDeg.y * Math.PI) / 180,
            (xf.rotationDeg.z * Math.PI) / 180,
            "XYZ",
          ),
        ),
        new THREE.Vector3(xf.scale.x, xf.scale.y, xf.scale.z),
      );
    }

    let minX = Infinity,
      maxX = -Infinity,
      minY = Infinity,
      maxY = -Infinity;
    for (const v of verts) {
      if (v.x < minX) minX = v.x;
      if (v.x > maxX) maxX = v.x;
      if (v.y < minY) minY = v.y;
      if (v.y > maxY) maxY = v.y;
    }

    // The board lies in the kernel XY plane; the centered slab's top surface is
    // at +thickness/2 with normal +Z. Frame from the top so the swing lands on
    // a familiar top-down-ish view that the user can then orbit away from.
    const z = (pcb.outline.thickness ?? 1.6) / 2;
    const c = new THREE.Vector3((minX + maxX) / 2, (minY + maxY) / 2, z).applyMatrix4(mat);
    const centroid = { x: c.x, y: c.y, z: c.z };
    const vertices = verts.map((v) => {
      const p = new THREE.Vector3(v.x, v.y, z).applyMatrix4(mat);
      return { x: p.x, y: p.y, z: p.z };
    });
    const n = new THREE.Vector3(0, 0, 1).transformDirection(mat);

    firedRef.current = true;
    window.dispatchEvent(
      new CustomEvent("vcad:face-selected", {
        detail: { normal: { x: n.x, y: n.y, z: n.z }, centroid, vertices },
      }),
    );
  }, [boardNodeId]);
}
