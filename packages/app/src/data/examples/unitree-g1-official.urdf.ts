// URDF source bundled via Vite `?raw`. The "official" variant is the
// upstream URDF from `unitreerobotics/unitree_ros`; mesh references inside
// the URDF can't be resolved from the browser filesystem so links fall
// back to 1cm placeholder cubes — joint topology + authored inertials
// still flow through.
import urdfText from "../../../../../examples/unitree-g1-official.urdf?raw";

import type { Example } from "./index";

export const unitreeG1OfficialExample: Example = {
  id: "unitree-g1-official",
  name: "Unitree G1 (official, no meshes)",
  description:
    "Upstream Unitree G1 23-DOF URDF. Visuals appear as placeholder cubes since the browser can't reach the STL meshes — joint topology and inertials are exact, so simulation behaves like the real robot.",
  difficulty: "advanced",
  features: ["robotics", "physics", "urdf", "humanoid", "official"],
  urdf: { urdfText, name: "unitree-g1-official.urdf" },
};
