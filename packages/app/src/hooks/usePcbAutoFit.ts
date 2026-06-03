import { useEffect, useRef } from "react";
import { useCoreElectronicsStore, useDocumentStore, getNodePcb } from "@vcad/core";

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

    const pcb = getNodePcb(useDocumentStore.getState().document, boardNodeId);
    const verts = pcb?.outline?.vertices;
    if (!pcb || !verts || verts.length < 3) return;

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

    // The board lies in the kernel XY plane; its top surface normal is +Z.
    // Frame from the top so the swing lands on a familiar top-down-ish view
    // that the user can then orbit away from.
    const z = pcb.outline.thickness ?? 1.6;
    const centroid = { x: (minX + maxX) / 2, y: (minY + maxY) / 2, z };
    const vertices = verts.map((v) => ({ x: v.x, y: v.y, z }));

    firedRef.current = true;
    window.dispatchEvent(
      new CustomEvent("vcad:face-selected", {
        detail: { normal: { x: 0, y: 0, z: 1 }, centroid, vertices },
      }),
    );
  }, [boardNodeId]);
}
