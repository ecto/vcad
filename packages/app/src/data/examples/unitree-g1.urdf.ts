// Official Unitree G1 (23-DOF) URDF. The XML is inlined via Vite `?raw`,
// and the STL meshes referenced inside it are bundled as separate URL
// assets — the example loader fetches each mesh on click, parses it with
// three.js, and inlines the triangle data into the document before the
// editor sees it. See lib/urdf-meshes.ts for the loader pipeline.
import urdfText from "../../../../../examples/unitree-g1.urdf?raw";

import type { Example } from "./index";

// Vite eagerly resolves each STL file to a hashed URL at build time. Using
// `import.meta.glob` keeps the manifest in sync with whatever's actually
// vendored under examples/meshes/g1/.
const meshUrlsRaw = import.meta.glob(
  "../../../../../examples/meshes/g1/*.STL",
  {
    eager: true,
    query: "?url",
    import: "default",
  },
) as Record<string, string>;

// The URDF writes `<mesh filename="meshes/foo.STL">`; flip the keys so the
// example loader can look them up directly by the URDF reference value.
const meshes: Record<string, string> = {};
for (const [absPath, url] of Object.entries(meshUrlsRaw)) {
  const basename = absPath.split("/").pop()!;
  meshes[`meshes/${basename}`] = url;
}

export const unitreeG1Example: Example = {
  id: "unitree-g1",
  name: "Unitree G1 (humanoid)",
  description:
    "Unitree G1 23-DOF humanoid — official URDF rendered with the upstream STL meshes. Click Simulate ▶ to drop it into gravity.",
  difficulty: "advanced",
  features: ["robotics", "physics", "urdf", "humanoid"],
  urdf: { urdfText, name: "unitree-g1.urdf", meshes },
};
