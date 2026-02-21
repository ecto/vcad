import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Coil spring — circular profile swept along a helix path
// Z is UP (standard CAD convention)
//
// LEARNING POINTS:
// - Sketch2D with Arc segments to define circular wire cross-section
// - Sweep operation with Helix path to create coiled geometry
// - Helix parameters: radius (coil diameter), pitch (spacing), turns
// - Arc segments create smoother circles than line approximations
const wireRadius = 1.5; // mm

// Build a circle from arc segments - each arc becomes one bilinear quad in sweep
// More arcs = rounder cross-section (16 gives good quality, 32 for high quality)
const numArcs = 16;
type ArcSegment = {
  type: "Arc";
  start: { x: number; y: number };
  end: { x: number; y: number };
  center: { x: number; y: number };
  ccw: boolean;
};
const circleSegments: ArcSegment[] = [];
for (let i = 0; i < numArcs; i++) {
  const angle1 = (2 * Math.PI * i) / numArcs;
  const angle2 = (2 * Math.PI * (i + 1)) / numArcs;
  circleSegments.push({
    type: "Arc",
    start: { x: wireRadius * Math.cos(angle1), y: wireRadius * Math.sin(angle1) },
    end: { x: wireRadius * Math.cos(angle2), y: wireRadius * Math.sin(angle2) },
    center: { x: 0, y: 0 },
    ccw: true,
  });
}

const document: Document = {
  version: "0.1",
  nodes: {
    // === SKETCH PROFILE ===
    // Circular cross-section for the spring wire (1.5mm radius)
    "1": {
      id: 1,
      name: "Wire Cross-Section",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 0 },
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 1, z: 0 },
        segments: circleSegments,
      },
    },

    // === HELIX SWEEP ===
    // Sweep the wire profile along a helical path
    "2": {
      id: 2,
      name: "Helix Sweep",
      op: {
        type: "Sweep",
        sketch: 1,
        path: {
          type: "Helix",
          radius: 10,   // 10mm helix radius (coil outer diameter = 20mm + wire)
          pitch: 8,     // 8mm vertical distance per turn
          height: 50,   // 50mm total spring height
          turns: 6.25,  // Number of complete turns (height / pitch)
        },
      },
    },

    // === FINAL TRANSFORMS ===
    "3": { id: 3, name: "Scale", op: { type: "Scale", child: 2, factor: { x: 1, y: 1, z: 1 } } },
    "4": { id: 4, name: "Rotate", op: { type: "Rotate", child: 3, angles: { x: 0, y: 0, z: 0 } } },
    "5": { id: 5, name: "Spring Coil", op: { type: "Translate", child: 4, offset: { x: 0, y: 0, z: 0 } } },
  },
  materials: {
    silver: {
      name: "Metallic Silver",
      color: [0.75, 0.75, 0.78],
      metallic: 0.9,
      roughness: 0.25,
    },
  },
  part_materials: {},
  roots: [{ root: 5, material: "silver" }],
  // === ASSEMBLY STRUCTURE ===
  partDefs: {
    spring: {
      id: "spring",
      name: "Spring Coil",
      root: 5,
      defaultMaterial: "silver",
    },
  },
  instances: [
    {
      id: "spring-1",
      partDefId: "spring",
      name: "Spring Coil",
      transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } },
    },
  ],
  groundInstanceId: "spring-1",
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Spring Coil",
    kind: "sweep",
    sketchNodeId: 1,
    sweepNodeId: 2,
    scaleNodeId: 3,
    rotateNodeId: 4,
    translateNodeId: 5,
  },
];

export const springExample: Example = {
  id: "spring",
  name: "Spring",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 6,
    nextPartNum: 2,
  },
};
