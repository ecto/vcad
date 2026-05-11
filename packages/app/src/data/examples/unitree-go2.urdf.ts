// Official Unitree Go2 quadruped URDF. The XML is inlined via Vite `?raw`,
// and the Collada (.dae) meshes it references via `package://` URIs are
// bundled as separate URL assets — the example loader fetches each mesh
// on click, parses it with three.js, and inlines the triangle data into
// the document before the editor sees it. See lib/urdf-meshes.ts.
import urdfText from "../../../../../examples/unitree-go2.urdf?raw";

import type { Example } from "./index";

const meshUrlsRaw = import.meta.glob(
  "../../../../../examples/meshes/go2/*.dae",
  {
    eager: true,
    query: "?url",
    import: "default",
  },
) as Record<string, string>;

// The URDF writes `<mesh filename="package://go2_description/dae/foo.dae">`.
// Register each asset under that exact form so the inliner can find it.
const meshes: Record<string, string> = {};
for (const [absPath, url] of Object.entries(meshUrlsRaw)) {
  const basename = absPath.split("/").pop()!;
  meshes[`package://go2_description/dae/${basename}`] = url;
}

export const unitreeGo2Example: Example = {
  id: "unitree-go2",
  name: "Unitree Go2 (quadruped)",
  description:
    "Unitree Go2 quadruped — official URDF rendered with the upstream Collada meshes. Click Simulate ▶ to watch it stand under gravity.",
  difficulty: "advanced",
  features: ["robotics", "physics", "urdf", "quadruped"],
  urdf: { urdfText, name: "unitree-go2.urdf", meshes },
};
