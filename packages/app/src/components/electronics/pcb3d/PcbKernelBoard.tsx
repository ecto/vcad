/**
 * Unified kernel-mesh board renderer.
 *
 * Renders the board exactly as the MCP viewer does — from the kernel's
 * `pcb_preview_meshes` (laminate + translucent soldermask shells + raw copper
 * under the mask + exposed ENIG pads/vias) — so the app and the agent surface
 * can never drift apart visually. The kernel is the single source of board
 * geometry; this component owns no tessellation of its own.
 *
 * Editor affordances stay app-side:
 *  - Selection / hovered-net highlights are extracted from the kernel's
 *    per-entity triangle ranges (`PcbPreviewEntity`) as index-subset overlay
 *    geometries sharing the copper vertex buffers — no duplicated math.
 *  - Layer visibility and stackup explosion use the per-mesh `layer` tag.
 *  - Picking is unchanged: PcbScene's invisible interaction plane hit-tests
 *    board data analytically.
 *  - Footprint chrome (silk/courtyard/ref text) and component bodies keep
 *    their dedicated components, so the kernel's `silkscreen`/`component`
 *    meshes are skipped here.
 *
 * Re-meshing is debounced: edits (e.g. a footprint drag, which mutates the
 * document per pointer-move) coalesce into one WASM tessellation per
 * REMESH_DEBOUNCE_MS, with the previous board shown until the new one lands.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import { useThree } from "@react-three/fiber";
import type { Pcb, PcbLayer } from "@vcad/ir";
import {
  pcbPreviewMeshes,
  type PcbPreviewMesh,
  type PcbPreviewEntity,
} from "@vcad/engine";
import { layerZOffset, isLayerVisible } from "./pcb-geometry";
import type { LayerConfig, ElectronicsSelection } from "@/stores/electronics-store";


const REMESH_DEBOUNCE_MS = 120;
const ACCENT = new THREE.Color("#3b82f6");

// Roles rendered by dedicated interactive components, not here.
const SKIPPED_ROLES = new Set(["component", "silkscreen"]);
// Roles that make up the board body (suppressed when showBoard=false because
// the board's kernel part already renders in the main scene).
const BOARD_ROLES = new Set(["laminate", "substrate", "mask"]);

interface Props {
  pcb: Pcb;
  layers: LayerConfig[];
  activeNet: string | null;
  hoveredNet: string | null;
  selection: ElectronicsSelection;
  explosion: number;
  showBoard: boolean;
}

interface BuiltMesh {
  key: string;
  role: string;
  layer: string | undefined;
  geometry: THREE.BufferGeometry;
  material: THREE.Material;
  entities: PcbPreviewEntity[];
}

/** Stackup-explosion z-shift for a layer-tagged mesh (0 when unexploded —
 *  kernel meshes are already at their physical z). */
function explodeShift(layer: string | undefined, explosion: number): number {
  if (!layer || explosion <= 0) return 0;
  const l = layer as PcbLayer;
  return layerZOffset(l, explosion) - layerZOffset(l, 0);
}

/** Does this entity match the current selection? */
function entitySelected(e: PcbPreviewEntity, pcb: Pcb, selection: ElectronicsSelection): boolean {
  switch (selection.type) {
    case "trace":
      return e.kind === "trace" && e.index === selection.idx;
    case "via":
      return e.kind === "via" && e.index === selection.idx;
    case "pad": {
      if (e.kind !== "pad" || e.footprint === undefined) return false;
      const fp = pcb.footprints[e.footprint];
      return (
        fp?.ref === selection.fpRef &&
        fp?.pads[e.index]?.number === selection.padNum
      );
    }
    case "footprint":
    case "component":
      return (
        e.kind === "pad" &&
        e.footprint !== undefined &&
        pcb.footprints[e.footprint]?.ref === selection.ref
      );
    default:
      return false;
  }
}

export function PcbKernelBoard({
  pcb,
  layers,
  activeNet,
  hoveredNet,
  selection,
  explosion,
  showBoard,
}: Props) {
  const { invalidate } = useThree();
  const [meshes, setMeshes] = useState<PcbPreviewMesh[]>([]);
  const generation = useRef(0);

  // Debounced kernel re-mesh whenever board data changes. Stale async results
  // (an older tessellation resolving after a newer edit) are dropped by the
  // generation counter; the previous board stays visible until the new one
  // lands, so edits never blink through an empty scene.
  useEffect(() => {
    const gen = ++generation.current;
    const handle = setTimeout(() => {
      void pcbPreviewMeshes(pcb)
        .then((m) => {
          if (generation.current !== gen) return;
          if (import.meta.env.DEV) {
            console.debug(
              "[PcbKernelBoard] meshes:",
              m.map((x) => `${x.role}/${x.layer ?? "-"}:${x.indices.length / 3}`).join(" "),
            );
          }
          setMeshes(m);
          invalidate();
        })
        .catch((e) => console.error("[PcbKernelBoard] preview meshes failed:", e));
    }, REMESH_DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [pcb, invalidate]);

  // Kernel buffers → three geometries/materials. Rebuilt only when a new
  // tessellation lands, not on highlight/visibility changes.
  const built = useMemo<BuiltMesh[]>(() => {
    return meshes
      .filter((m) => !SKIPPED_ROLES.has(m.role))
      .map((m, i) => {
        const geometry = new THREE.BufferGeometry();
        geometry.setAttribute(
          "position",
          new THREE.Float32BufferAttribute(m.positions, 3),
        );
        if (m.normals.length === m.positions.length) {
          geometry.setAttribute(
            "normal",
            new THREE.Float32BufferAttribute(m.normals, 3),
          );
        } else {
          geometry.computeVertexNormals();
        }
        geometry.setIndex(m.indices);

        const alpha = m.alpha ?? 1;
        const material = new THREE.MeshPhysicalMaterial({
          color: new THREE.Color(m.color[0], m.color[1], m.color[2]),
          metalness: m.metalness,
          roughness: m.roughness,
          // Match the MCP viewer's tamed IBL response so both surfaces
          // render the same board (its tameMaterials floors these).
          envMapIntensity: 0.5,
          clearcoat: m.clearcoat ?? 0,
          clearcoatRoughness: m.clearcoat_roughness ?? 0,
          ...(alpha < 1
            ? {
                transparent: true,
                opacity: alpha,
                depthWrite: false,
                side: THREE.DoubleSide,
              }
            : {}),
          ...((m.emissive?.[0] ?? 0) > 0 ||
          (m.emissive?.[1] ?? 0) > 0 ||
          (m.emissive?.[2] ?? 0) > 0
            ? {
                emissive: new THREE.Color(
                  m.emissive![0],
                  m.emissive![1],
                  m.emissive![2],
                ),
                emissiveIntensity: 1,
              }
            : {}),
        });

        return {
          key: `${m.role}-${m.layer ?? "all"}-${i}`,
          role: m.role,
          layer: m.layer,
          geometry,
          material,
          entities: m.entities ?? [],
        };
      });
  }, [meshes]);

  useEffect(
    () => () =>
      built.forEach((b) => {
        b.geometry.dispose();
        b.material.dispose();
      }),
    [built],
  );

  // Highlight overlays: index subsets (shared vertex buffers) of the copper
  // entities on the active/hovered net or under the selection.
  const overlays = useMemo(() => {
    const out: { key: string; geometry: THREE.BufferGeometry }[] = [];
    for (const b of built) {
      if (b.entities.length === 0) continue;
      const wanted = b.entities.filter(
        (e) =>
          (activeNet != null && e.net === activeNet) ||
          (hoveredNet != null && e.net === hoveredNet) ||
          entitySelected(e, pcb, selection),
      );
      if (wanted.length === 0) continue;
      const src = b.geometry.getIndex()!;
      let total = 0;
      for (const e of wanted) total += e.count;
      const idx = new Uint32Array(total);
      let o = 0;
      for (const e of wanted) {
        for (let k = 0; k < e.count; k++) idx[o++] = src.getX(e.start + k);
      }
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", b.geometry.getAttribute("position"));
      const normal = b.geometry.getAttribute("normal");
      if (normal) geometry.setAttribute("normal", normal);
      geometry.setIndex(new THREE.BufferAttribute(idx, 1));
      out.push({ key: `hl-${b.key}`, geometry });
    }
    return out;
  }, [built, pcb, activeNet, hoveredNet, selection]);

  useEffect(
    () => () => overlays.forEach((o) => o.geometry.dispose()),
    [overlays],
  );

  // Overlay geometries index into per-mesh vertex buffers, so each overlay
  // must ride at the same explosion offset as its source mesh — key both off
  // the source mesh key.
  const overlayShift = useMemo(() => {
    const map = new Map<string, number>();
    for (const b of built) map.set(`hl-${b.key}`, explodeShift(b.layer, explosion));
    return map;
  }, [built, explosion]);

  return (
    <group>
      {built.map((b) => {
        if (BOARD_ROLES.has(b.role) && !showBoard) return null;
        if (b.layer && !isLayerVisible(layers, b.layer as PcbLayer)) return null;
        return (
          <mesh
            key={b.key}
            geometry={b.geometry}
            material={b.material}
            position={[0, 0, explodeShift(b.layer, explosion)]}
            // The translucent mask must blend over its own board's copper
            // before other transparents; default sort order is fine, but
            // never let it write depth (set on the material above).
            renderOrder={b.role === "mask" ? 1 : 0}
          />
        );
      })}
      {overlays.map((o) => (
        <mesh
          key={o.key}
          geometry={o.geometry}
          position={[0, 0, overlayShift.get(o.key) ?? 0]}
          renderOrder={2}
        >
          <meshStandardMaterial
            color={ACCENT}
            emissive={ACCENT}
            emissiveIntensity={0.55}
            roughness={0.35}
            metalness={0.2}
            polygonOffset
            polygonOffsetFactor={-2}
            polygonOffsetUnits={-2}
          />
        </mesh>
      ))}
    </group>
  );
}

export default PcbKernelBoard;
