// URDF source bundled via Vite `?raw`. Loading happens through the
// engine's `importUrdf` at click time — see App.tsx onLoadExample.
import urdfText from "../../../../../examples/unitree-g1.urdf?raw";

import type { Example } from "./index";

export const unitreeG1Example: Example = {
  id: "unitree-g1",
  name: "Unitree G1 (humanoid)",
  description:
    "23-DOF humanoid robot. Hand-authored primitive geometry, full joint topology and authored inertials. Click Simulate ▶ to drop it into gravity.",
  difficulty: "advanced",
  features: ["robotics", "physics", "urdf", "humanoid"],
  urdf: { urdfText, name: "unitree-g1.urdf" },
};
