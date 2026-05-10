// URDF source bundled via Vite `?raw`. Loading happens through the
// engine's `importUrdf` at click time — see App.tsx onLoadExample.
import urdfText from "../../../../../examples/unitree-go2.urdf?raw";

import type { Example } from "./index";

export const unitreeGo2Example: Example = {
  id: "unitree-go2",
  name: "Unitree Go2 (quadruped)",
  description:
    "12-DOF quadruped robot. Hand-authored primitive geometry, full joint topology and authored inertials. Click Simulate ▶ to drop it into gravity.",
  difficulty: "advanced",
  features: ["robotics", "physics", "urdf", "quadruped"],
  urdf: { urdfText, name: "unitree-go2.urdf" },
};
