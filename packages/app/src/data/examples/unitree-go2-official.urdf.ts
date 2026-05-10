// URDF source bundled via Vite `?raw`. Upstream Go2 URDF; mesh refs are
// DAE files that vcad doesn't load, so all links fall back to placeholder
// cubes. Joint topology and authored inertials still flow through.
import urdfText from "../../../../../examples/unitree-go2-official.urdf?raw";

import type { Example } from "./index";

export const unitreeGo2OfficialExample: Example = {
  id: "unitree-go2-official",
  name: "Unitree Go2 (official, no meshes)",
  description:
    "Upstream Unitree Go2 URDF (41 joints incl. rotors / sensors). Visuals are placeholder cubes — the URDF references DAE meshes that vcad doesn't load yet — but joint topology and inertials are exact.",
  difficulty: "advanced",
  features: ["robotics", "physics", "urdf", "quadruped", "official"],
  urdf: { urdfText, name: "unitree-go2-official.urdf" },
};
