/**
 * Shared "what is there to export?" view of an evaluated scene.
 *
 * A document authored as an ASSEMBLY has `roots: []` — all geometry lives in
 * `partDefs` + `instances`, placed by the joint tree — so anything that walks
 * `scene.parts` alone sees an empty document. This module folds both shapes
 * into one list of export units, mirroring the traversal `render_view` /
 * `get_preview_glb` / `inspect_cad` already use (a part-local mesh plus the
 * FK-solved world transform).
 */

import { transformMesh } from "@vcad/engine";
import type { EvaluatedScene, TriangleMesh } from "@vcad/engine";

/** World placement of an instance's part-local mesh (Euler XYZ degrees). */
export interface UnitTransform {
  translation: { x: number; y: number; z: number };
  rotation: { x: number; y: number; z: number };
  scale: { x: number; y: number; z: number };
}

/** One exportable piece of geometry: a scene root part, or an assembly
 *  instance (part-local `mesh` + world `transform`). */
export interface ExportUnit {
  /** glTF node name — `"<part_id>:<name>"` for roots,
   *  `"<instanceId>:<name>"` for assembly instances. */
  name: string;
  /** Part-LOCAL geometry. Apply `transform` (or {@link worldMesh}) for world space. */
  mesh: TriangleMesh;
  /** World placement; absent for root parts (geometry is already world-space). */
  transform?: UnitTransform;
  /** Geometry-dedup key (partDefId) so repeated instances share one glTF mesh. */
  meshKey?: string;
}

/**
 * Flatten an evaluated scene into export units: root parts first (index-aligned
 * with `partLabels`), then assembly instances.
 */
export function sceneExportUnits(
  scene: EvaluatedScene,
  partLabels?: string[],
): ExportUnit[] {
  const units: ExportUnit[] = scene.parts.map((part, i) => ({
    name: partLabels?.[i] ?? `part_${i}`,
    mesh: part.mesh,
  }));

  for (const inst of scene.instances ?? []) {
    units.push({
      name: `${inst.instanceId}:${inst.name ?? ""}`,
      mesh: inst.mesh,
      transform: inst.transform,
      meshKey: inst.partDefId,
    });
  }

  return units;
}

/** The unit's geometry in world space (transform baked into the vertices). */
export function worldMesh(unit: ExportUnit): TriangleMesh {
  if (!unit.transform) return unit.mesh;
  return transformMesh(unit.mesh, {
    translate: unit.transform.translation,
    rotate: unit.transform.rotation,
    scale: unit.transform.scale,
  });
}
