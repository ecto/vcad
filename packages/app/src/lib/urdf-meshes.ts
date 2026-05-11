import { STLLoader } from "three/examples/jsm/loaders/STLLoader.js";
import { ColladaLoader } from "three/examples/jsm/loaders/ColladaLoader.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import type { BufferGeometry } from "three";
import { Mesh, Matrix4 } from "three";
import type { Document, Node, NodeId } from "@vcad/ir";

export interface LoadedMesh {
  positions: number[];
  indices: number[];
  normals?: number[];
}

/**
 * Map from URDF `<mesh filename="...">` value (or its basename / package
 * URI tail) to parsed mesh data. URDF authors are inconsistent about how
 * they reference meshes — relative paths, `package://`, sometimes mixed
 * within one file — so the loader stores each mesh under several aliases
 * and lookup tries the full filename first, then progressively shorter
 * variants.
 */
export type UrdfMeshMap = Map<string, LoadedMesh>;

const stlLoader = new STLLoader();
const colladaLoader = new ColladaLoader();
const gltfLoader = new GLTFLoader();

async function fetchBuffer(url: string): Promise<ArrayBuffer> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`Failed to fetch ${url}: ${res.status} ${res.statusText}`);
  }
  return res.arrayBuffer();
}

function geometryToLoadedMesh(geom: BufferGeometry, scale = 1000): LoadedMesh {
  // Meshes from Unitree / ROS are in metres; vcad uses mm. The kernel
  // multiplies URDF dimensions by 1000 in `geometry_to_csg`, so we do
  // the same for inline mesh vertices.
  const posAttr = geom.getAttribute("position");
  const positions: number[] = new Array(posAttr.count * 3);
  for (let i = 0; i < posAttr.count; i++) {
    positions[i * 3] = posAttr.getX(i) * scale;
    positions[i * 3 + 1] = posAttr.getY(i) * scale;
    positions[i * 3 + 2] = posAttr.getZ(i) * scale;
  }

  let indices: number[];
  if (geom.index) {
    indices = Array.from(geom.index.array);
  } else {
    indices = new Array(posAttr.count);
    for (let i = 0; i < posAttr.count; i++) indices[i] = i;
  }

  let normals: number[] | undefined;
  const normAttr = geom.getAttribute("normal");
  if (normAttr) {
    normals = new Array(normAttr.count * 3);
    for (let i = 0; i < normAttr.count; i++) {
      normals[i * 3] = normAttr.getX(i);
      normals[i * 3 + 1] = normAttr.getY(i);
      normals[i * 3 + 2] = normAttr.getZ(i);
    }
  }

  return { positions, indices, normals };
}

async function parseStl(buf: ArrayBuffer): Promise<LoadedMesh> {
  const geom = stlLoader.parse(buf);
  geom.computeVertexNormals();
  return geometryToLoadedMesh(geom);
}

/**
 * Collada files often hold a scene with multiple meshes (visual links
 * sometimes pack several rigid pieces). Bake every Mesh node's geometry
 * into one combined triangle soup.
 *
 * The transform we apply is each mesh's matrix RELATIVE TO the DAE scene
 * root, deliberately ignoring whatever rotation the loader put on the
 * scene itself to convert the file's `<up_axis>` into three.js's Y-up
 * convention. URDF and the vcad kernel both expect mesh vertices in the
 * file's authored Z-up frame — exactly the frame the node matrices
 * inside `<library_visual_scenes>` are written in — so baking three.js's
 * Y-up conversion would tip every leg / arm by 90° relative to the link
 * frame the URDF positions it in.
 */
async function parseCollada(
  buf: ArrayBuffer,
  url: string,
): Promise<LoadedMesh> {
  const text = new TextDecoder().decode(buf);
  const dae = colladaLoader.parse(text, url.slice(0, url.lastIndexOf("/") + 1));
  // Drop the loader's up-axis correction so descendant `matrixWorld`s are
  // expressed in the DAE's native frame.
  dae.scene.matrix.identity();
  dae.scene.matrixAutoUpdate = false;
  dae.scene.updateMatrixWorld(true);

  const positions: number[] = [];
  const indices: number[] = [];
  const normals: number[] = [];
  let baseIndex = 0;
  const tmp = new Matrix4();

  dae.scene.traverse((obj) => {
    if (!(obj instanceof Mesh)) return;
    const geom = obj.geometry as BufferGeometry;
    const posAttr = geom.getAttribute("position");
    if (!posAttr) return;
    const normAttr = geom.getAttribute("normal");
    tmp.copy(obj.matrixWorld);
    const count = posAttr.count;
    for (let i = 0; i < count; i++) {
      const x = posAttr.getX(i);
      const y = posAttr.getY(i);
      const z = posAttr.getZ(i);
      const wx = tmp.elements[0] * x + tmp.elements[4] * y + tmp.elements[8] * z + tmp.elements[12];
      const wy = tmp.elements[1] * x + tmp.elements[5] * y + tmp.elements[9] * z + tmp.elements[13];
      const wz = tmp.elements[2] * x + tmp.elements[6] * y + tmp.elements[10] * z + tmp.elements[14];
      // m → mm
      positions.push(wx * 1000, wy * 1000, wz * 1000);
      if (normAttr) {
        const nx = normAttr.getX(i);
        const ny = normAttr.getY(i);
        const nz = normAttr.getZ(i);
        const rx = tmp.elements[0] * nx + tmp.elements[4] * ny + tmp.elements[8] * nz;
        const ry = tmp.elements[1] * nx + tmp.elements[5] * ny + tmp.elements[9] * nz;
        const rz = tmp.elements[2] * nx + tmp.elements[6] * ny + tmp.elements[10] * nz;
        normals.push(rx, ry, rz);
      }
    }
    if (geom.index) {
      const idx = geom.index.array;
      for (let i = 0; i < idx.length; i++) indices.push(baseIndex + idx[i]!);
    } else {
      for (let i = 0; i < count; i++) indices.push(baseIndex + i);
    }
    baseIndex += count;
  });

  return {
    positions,
    indices,
    normals: normals.length === positions.length ? normals : undefined,
  };
}

async function parseGlb(buf: ArrayBuffer): Promise<LoadedMesh> {
  return new Promise((resolve, reject) => {
    gltfLoader.parse(
      buf,
      "",
      (gltf) => {
        const positions: number[] = [];
        const indices: number[] = [];
        const normals: number[] = [];
        let baseIndex = 0;
        const tmp = new Matrix4();
        gltf.scene.updateMatrixWorld(true);
        gltf.scene.traverse((obj) => {
          if (!(obj instanceof Mesh)) return;
          const geom = obj.geometry as BufferGeometry;
          const posAttr = geom.getAttribute("position");
          if (!posAttr) return;
          const normAttr = geom.getAttribute("normal");
          tmp.copy(obj.matrixWorld);
          for (let i = 0; i < posAttr.count; i++) {
            const x = posAttr.getX(i);
            const y = posAttr.getY(i);
            const z = posAttr.getZ(i);
            const wx = tmp.elements[0] * x + tmp.elements[4] * y + tmp.elements[8] * z + tmp.elements[12];
            const wy = tmp.elements[1] * x + tmp.elements[5] * y + tmp.elements[9] * z + tmp.elements[13];
            const wz = tmp.elements[2] * x + tmp.elements[6] * y + tmp.elements[10] * z + tmp.elements[14];
            positions.push(wx * 1000, wy * 1000, wz * 1000);
            if (normAttr) {
              normals.push(normAttr.getX(i), normAttr.getY(i), normAttr.getZ(i));
            }
          }
          if (geom.index) {
            const idx = geom.index.array;
            for (let i = 0; i < idx.length; i++) indices.push(baseIndex + idx[i]!);
          } else {
            for (let i = 0; i < posAttr.count; i++) indices.push(baseIndex + i);
          }
          baseIndex += posAttr.count;
        });
        resolve({
          positions,
          indices,
          normals: normals.length === positions.length ? normals : undefined,
        });
      },
      reject,
    );
  });
}

/**
 * Fetch and parse a single mesh file by URL. The file extension drives
 * loader selection (STL / DAE / GLB / GLTF).
 */
export async function loadMeshFromUrl(url: string): Promise<LoadedMesh> {
  const ext = url.split(".").pop()?.toLowerCase() ?? "";
  const buf = await fetchBuffer(url);
  if (ext === "stl") return parseStl(buf);
  if (ext === "dae") return parseCollada(buf, url);
  if (ext === "glb" || ext === "gltf") return parseGlb(buf);
  throw new Error(`Unsupported mesh extension: ${ext} (${url})`);
}

/**
 * Load every mesh file in `urls` in parallel and build a lookup table
 * keyed by URDF `<mesh filename>` value. Each mesh is registered under
 * the full filename and (for `package://NAME/path/foo.STL`) the
 * stripped-prefix variant `NAME/path/foo.STL`, plus the bare basename.
 * This matches the loose conventions in published URDFs.
 */
export async function loadUrdfMeshes(
  urls: Record<string, string>,
): Promise<UrdfMeshMap> {
  const map: UrdfMeshMap = new Map();
  const entries = Object.entries(urls);

  await Promise.all(
    entries.map(async ([urdfRef, url]) => {
      const mesh = await loadMeshFromUrl(url);
      // Register under the URDF reference the example author gave us...
      map.set(urdfRef, mesh);
      // ...and a few common aliases so the post-processor can match a
      // mesh whether the URDF writes "meshes/foo.STL", "foo.STL", or
      // "package://x/foo.STL".
      const basename = urdfRef.split("/").pop();
      if (basename && basename !== urdfRef) map.set(basename, mesh);
      const stripped = urdfRef.replace(/^package:\/\/[^/]+\//, "");
      if (stripped !== urdfRef) map.set(stripped, mesh);
    }),
  );

  return map;
}

/**
 * Find every `MeshImport` node in the document and replace it with an
 * `ImportedMesh` carrying the loaded triangle data. The URDF importer
 * emits `MeshImport { path: <urdf filename> }` when it can't resolve the
 * filename against a filesystem; this swap turns those references into
 * inline meshes the renderer can draw.
 */
export function inlineMeshImports(doc: Document, meshes: UrdfMeshMap): void {
  let swapped = 0;
  let unresolved = 0;
  for (const key of Object.keys(doc.nodes)) {
    const node = doc.nodes[key] as Node;
    // The IR's `MeshImport` variant serializes as `"type": "mesh_import"`
    // (the only CsgOp that uses a serde rename). The TS `CsgOp` union
    // doesn't include this variant since it's only ever produced by the
    // URDF importer en route to being swapped to `ImportedMesh`, so we
    // sidestep the discriminator narrowing by casting to a structural
    // shape.
    const opLoose = node.op as unknown as {
      type: string;
      path?: string;
      scale?: { x: number; y: number; z: number } | null;
    };
    if (opLoose.type !== "mesh_import" && opLoose.type !== "MeshImport") continue;
    const path = opLoose.path ?? "";
    const mesh = lookupMesh(meshes, path);
    if (!mesh) {
      unresolved++;
      continue;
    }
    const scale = opLoose.scale;
    const positions = mesh.positions;
    const scaled =
      scale && (scale.x !== 1 || scale.y !== 1 || scale.z !== 1)
        ? applyScale(positions, [scale.x, scale.y, scale.z])
        : positions;
    doc.nodes[key] = {
      id: node.id as NodeId,
      name: node.name,
      op: {
        type: "ImportedMesh",
        positions: scaled,
        indices: mesh.indices,
        normals: mesh.normals,
        source: path,
        // biome-ignore lint/suspicious/noExplicitAny: bridging IR shape
      } as any,
    };
    swapped++;
  }
  if (unresolved > 0) {
    console.warn(
      `[urdf-meshes] inlined ${swapped} mesh(es); ${unresolved} reference(s) had no matching asset`,
    );
  }
}

function lookupMesh(meshes: UrdfMeshMap, path: string): LoadedMesh | undefined {
  const direct = meshes.get(path);
  if (direct) return direct;
  const stripped = path.replace(/^package:\/\/[^/]+\//, "");
  const strippedHit = meshes.get(stripped);
  if (strippedHit) return strippedHit;
  const basename = path.split("/").pop();
  if (basename) {
    const baseHit = meshes.get(basename);
    if (baseHit) return baseHit;
  }
  return undefined;
}

function applyScale(positions: number[], scale: [number, number, number]): number[] {
  const out = new Array(positions.length);
  for (let i = 0; i < positions.length; i += 3) {
    out[i] = positions[i]! * scale[0];
    out[i + 1] = positions[i + 1]! * scale[1];
    out[i + 2] = positions[i + 2]! * scale[2];
  }
  return out;
}
