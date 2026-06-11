/**
 * inspect_cad tool — query geometry properties for an open session
 * document. Sums volume, surface area, mass (when materials carry
 * density), and the overall bounding box across every part.
 *
 * Per-part inspection (the chat surface's `inspect_part` and
 * `describe_scene`) is deferred to a follow-up — those need
 * scene-relative anchors and aren't yet routable through the IR-direct
 * MCP path.
 */

import type { Engine, TriangleMesh } from "@vcad/engine";
import { getSession } from "./session.js";

export const inspectCadSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description: "Session id from open_document.",
    },
  },
  required: ["document_id"],
};

interface BoundingBox {
  min: { x: number; y: number; z: number };
  max: { x: number; y: number; z: number };
}

/** Per-part mass information. */
interface PartMassInfo {
  name: string;
  volume_mm3: number;
  material: string;
  density_kg_m3?: number;
  mass_g?: number;
}

interface InspectResult {
  volume_mm3: number;
  surface_area_mm2: number;
  bounding_box: BoundingBox;
  center_of_mass: { x: number; y: number; z: number };
  triangles: number;
  parts: number;
  mass_g?: number;
  part_masses?: PartMassInfo[];
}

/** Calculate signed volume of a triangle with origin. */
function signedVolumeOfTriangle(
  p1: [number, number, number],
  p2: [number, number, number],
  p3: [number, number, number],
): number {
  return (
    (p1[0] * (p2[1] * p3[2] - p3[1] * p2[2]) -
      p2[0] * (p1[1] * p3[2] - p3[1] * p1[2]) +
      p3[0] * (p1[1] * p2[2] - p2[1] * p1[2])) /
    6.0
  );
}

/** Calculate area of a triangle. */
function triangleArea(
  p1: [number, number, number],
  p2: [number, number, number],
  p3: [number, number, number],
): number {
  // Cross product of edges
  const ax = p2[0] - p1[0];
  const ay = p2[1] - p1[1];
  const az = p2[2] - p1[2];
  const bx = p3[0] - p1[0];
  const by = p3[1] - p1[1];
  const bz = p3[2] - p1[2];

  const cx = ay * bz - az * by;
  const cy = az * bx - ax * bz;
  const cz = ax * by - ay * bx;

  return Math.sqrt(cx * cx + cy * cy + cz * cz) / 2.0;
}

/** Get vertex position from mesh. */
function getVertex(
  mesh: TriangleMesh,
  index: number,
): [number, number, number] {
  const i = index * 3;
  return [mesh.positions[i], mesh.positions[i + 1], mesh.positions[i + 2]];
}

/** Compute properties for a single mesh. */
function computeMeshProperties(mesh: TriangleMesh): {
  volume: number;
  area: number;
  bbox: BoundingBox;
  centroid: { x: number; y: number; z: number };
  triangles: number;
} {
  const numTriangles = mesh.indices.length / 3;

  let volume = 0;
  let area = 0;
  let cx = 0,
    cy = 0,
    cz = 0;
  // Area-weighted surface centroid — fallback when the signed-volume
  // integral is unreliable (open or inconsistently wound meshes, e.g.
  // thin sheet-metal bodies). Always lands inside the bounding box.
  let acx = 0,
    acy = 0,
    acz = 0;

  const bbox: BoundingBox = {
    min: { x: Infinity, y: Infinity, z: Infinity },
    max: { x: -Infinity, y: -Infinity, z: -Infinity },
  };

  for (let t = 0; t < numTriangles; t++) {
    const i0 = mesh.indices[t * 3];
    const i1 = mesh.indices[t * 3 + 1];
    const i2 = mesh.indices[t * 3 + 2];

    const p1 = getVertex(mesh, i0);
    const p2 = getVertex(mesh, i1);
    const p3 = getVertex(mesh, i2);

    // Volume via divergence theorem
    const v = signedVolumeOfTriangle(p1, p2, p3);
    volume += v;

    // Surface area
    const a = triangleArea(p1, p2, p3);
    area += a;

    // Centroid contribution (weighted by signed volume)
    cx += v * (p1[0] + p2[0] + p3[0]) / 4;
    cy += v * (p1[1] + p2[1] + p3[1]) / 4;
    cz += v * (p1[2] + p2[2] + p3[2]) / 4;

    // Area-weighted centroid contribution
    acx += (a * (p1[0] + p2[0] + p3[0])) / 3;
    acy += (a * (p1[1] + p2[1] + p3[1])) / 3;
    acz += (a * (p1[2] + p2[2] + p3[2])) / 3;

    // Bounding box
    for (const p of [p1, p2, p3]) {
      bbox.min.x = Math.min(bbox.min.x, p[0]);
      bbox.min.y = Math.min(bbox.min.y, p[1]);
      bbox.min.z = Math.min(bbox.min.z, p[2]);
      bbox.max.x = Math.max(bbox.max.x, p[0]);
      bbox.max.y = Math.max(bbox.max.y, p[1]);
      bbox.max.z = Math.max(bbox.max.z, p[2]);
    }
  }

  // Normalize centroid by volume. The volume-weighted centroid is only
  // valid for a closed, consistently wound mesh — on open meshes the
  // signed-tet contributions partially cancel and the division can throw
  // the centroid anywhere, including outside the bounding box (which is
  // geometrically impossible for a real COM). Validate against the bbox
  // and fall back to the area-weighted surface centroid when it fails.
  const absVolume = Math.abs(volume);
  let centroid: { x: number; y: number; z: number } | null = null;
  if (absVolume > 1e-10) {
    centroid = { x: cx / volume, y: cy / volume, z: cz / volume };
    const eps =
      1e-9 *
      Math.max(
        bbox.max.x - bbox.min.x,
        bbox.max.y - bbox.min.y,
        bbox.max.z - bbox.min.z,
        1,
      );
    const inBbox =
      centroid.x >= bbox.min.x - eps &&
      centroid.x <= bbox.max.x + eps &&
      centroid.y >= bbox.min.y - eps &&
      centroid.y <= bbox.max.y + eps &&
      centroid.z >= bbox.min.z - eps &&
      centroid.z <= bbox.max.z + eps;
    if (!inBbox) centroid = null;
  }
  if (centroid === null) {
    centroid =
      area > 0
        ? { x: acx / area, y: acy / area, z: acz / area }
        : { x: 0, y: 0, z: 0 };
  }

  return {
    volume: absVolume,
    area,
    bbox,
    centroid,
    triangles: numTriangles,
  };
}

export function inspectCad(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const args = (input ?? {}) as Record<string, unknown>;
  const documentId = String(args.document_id ?? "");
  const ir = getSession(documentId);

  // Evaluate the document
  const scene = engine.evaluate(ir);

  if (scene.parts.length === 0) {
    throw new Error("Document has no parts to inspect");
  }

  // Aggregate properties across all parts
  let totalVolume = 0;
  let totalArea = 0;
  let totalTriangles = 0;
  let totalMass = 0;
  let hasMassData = false;
  let weightedCx = 0,
    weightedCy = 0,
    weightedCz = 0;

  const bbox: BoundingBox = {
    min: { x: Infinity, y: Infinity, z: Infinity },
    max: { x: -Infinity, y: -Infinity, z: -Infinity },
  };

  const partMasses: PartMassInfo[] = [];

  // Find the root nodes to get part names
  const rootNameMap = new Map<number, string>();
  for (const root of ir.roots) {
    const node = ir.nodes[String(root.root)];
    if (node?.name) {
      rootNameMap.set(root.root, node.name);
    }
  }

  for (let i = 0; i < scene.parts.length; i++) {
    const part = scene.parts[i];
    const props = computeMeshProperties(part.mesh);

    totalVolume += props.volume;
    totalArea += props.area;
    totalTriangles += props.triangles;

    // Weight centroid by volume
    weightedCx += props.centroid.x * props.volume;
    weightedCy += props.centroid.y * props.volume;
    weightedCz += props.centroid.z * props.volume;

    // Expand bounding box
    bbox.min.x = Math.min(bbox.min.x, props.bbox.min.x);
    bbox.min.y = Math.min(bbox.min.y, props.bbox.min.y);
    bbox.min.z = Math.min(bbox.min.z, props.bbox.min.z);
    bbox.max.x = Math.max(bbox.max.x, props.bbox.max.x);
    bbox.max.y = Math.max(bbox.max.y, props.bbox.max.y);
    bbox.max.z = Math.max(bbox.max.z, props.bbox.max.z);

    // Compute mass if material has density
    const materialKey = part.material ?? "default";
    const material = ir.materials?.[materialKey];
    const density = material?.density;

    // Get part name from root or use index
    const rootEntry = ir.roots[i];
    const partName = rootEntry ? rootNameMap.get(rootEntry.root) ?? `part_${i + 1}` : `part_${i + 1}`;

    const partMassInfo: PartMassInfo = {
      name: partName,
      volume_mm3: Math.round(props.volume * 1000) / 1000,
      material: materialKey,
    };

    if (density) {
      // mass (kg) = volume (mm³) / 1e9 * density (kg/m³)
      // mass (g) = mass (kg) * 1000
      const massKg = (props.volume / 1e9) * density;
      const massG = massKg * 1000;
      partMassInfo.density_kg_m3 = density;
      partMassInfo.mass_g = Math.round(massG * 1000) / 1000;
      totalMass += massG;
      hasMassData = true;
    }

    partMasses.push(partMassInfo);
  }

  // Compute overall center of mass
  const com =
    totalVolume > 1e-10
      ? {
          x: weightedCx / totalVolume,
          y: weightedCy / totalVolume,
          z: weightedCz / totalVolume,
        }
      : { x: 0, y: 0, z: 0 };

  const result: InspectResult = {
    volume_mm3: Math.round(totalVolume * 1000) / 1000,
    surface_area_mm2: Math.round(totalArea * 1000) / 1000,
    bounding_box: {
      min: {
        x: Math.round(bbox.min.x * 1000) / 1000,
        y: Math.round(bbox.min.y * 1000) / 1000,
        z: Math.round(bbox.min.z * 1000) / 1000,
      },
      max: {
        x: Math.round(bbox.max.x * 1000) / 1000,
        y: Math.round(bbox.max.y * 1000) / 1000,
        z: Math.round(bbox.max.z * 1000) / 1000,
      },
    },
    center_of_mass: {
      x: Math.round(com.x * 1000) / 1000,
      y: Math.round(com.y * 1000) / 1000,
      z: Math.round(com.z * 1000) / 1000,
    },
    triangles: totalTriangles,
    parts: scene.parts.length,
  };

  // Add mass data if any materials have density
  if (hasMassData) {
    result.mass_g = Math.round(totalMass * 1000) / 1000;
    result.part_masses = partMasses;
  }

  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(result, null, 2),
      },
    ],
  };
}
