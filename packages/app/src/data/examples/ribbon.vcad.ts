import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Twisted Ribbon - demonstrates line sweep with twist
// A rectangular cross-section swept along a straight vertical path
// with 720 degrees of twist creating a double helix ribbon effect
//
// LEARNING POINTS:
// - Sketch2D with Line segments to define rectangular profile
// - Sweep operation with Line path (straight extrusion with control)
// - twist_angle parameter to rotate profile along path
// - Different from Extrude: allows twist and variable scale
const document: Document = {
  version: "0.1",
  nodes: {
    // === SKETCH PROFILE ===
    // Rectangular cross-section: 10mm wide x 2mm thick, centered at origin
    "1": {
      id: 1,
      name: "Ribbon Profile (10x2mm)",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 0 },
        x_dir: { x: 1, y: 0, z: 0 },  // Sketch X = World X
        y_dir: { x: 0, y: 1, z: 0 },  // Sketch Y = World Y
        segments: [
          // Closed rectangle (4 line segments)
          { type: "Line", start: { x: -5, y: -1 }, end: { x: 5, y: -1 } },   // Bottom
          { type: "Line", start: { x: 5, y: -1 }, end: { x: 5, y: 1 } },     // Right
          { type: "Line", start: { x: 5, y: 1 }, end: { x: -5, y: 1 } },     // Top
          { type: "Line", start: { x: -5, y: 1 }, end: { x: -5, y: -1 } },   // Left (closes)
        ],
      },
    },

    // === LINE SWEEP WITH TWIST ===
    // Sweep profile along Z axis with 720° rotation
    "2": {
      id: 2,
      name: "Twisted Line Sweep",
      op: {
        type: "Sweep",
        sketch: 1,
        path: {
          type: "Line",
          start: { x: 0, y: 0, z: 0 },
          end: { x: 0, y: 0, z: 80 },  // 80mm vertical path
        },
        twist_angle: 4 * Math.PI,  // 720° = 2 full rotations
        scale_start: 1.0,
        scale_end: 1.0,
      },
    },

    // === FINAL TRANSFORMS ===
    "3": { id: 3, name: "Scale", op: { type: "Scale", child: 2, factor: { x: 1, y: 1, z: 1 } } },
    "4": { id: 4, name: "Rotate", op: { type: "Rotate", child: 3, angles: { x: 0, y: 0, z: 0 } } },
    "5": { id: 5, name: "Twisted Ribbon", op: { type: "Translate", child: 4, offset: { x: 0, y: 0, z: -40 } } },
  },
  materials: {
    copper: {
      name: "Copper",
      color: [0.95, 0.64, 0.54],
      metallic: 0.9,
      roughness: 0.25,
    },
  },
  part_materials: {},
  roots: [{ root: 5, material: "copper" }],
  // === ASSEMBLY STRUCTURE ===
  partDefs: {
    ribbon: {
      id: "ribbon",
      name: "Twisted Ribbon",
      root: 5,
      defaultMaterial: "copper",
    },
  },
  instances: [
    {
      id: "ribbon-1",
      partDefId: "ribbon",
      name: "Twisted Ribbon",
      transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } },
    },
  ],
  groundInstanceId: "ribbon-1",
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Twisted Ribbon",
    kind: "sweep",
    sketchNodeId: 1,
    sweepNodeId: 2,
    scaleNodeId: 3,
    rotateNodeId: 4,
    translateNodeId: 5,
  },
];

export const ribbonExample: Example = {
  id: "ribbon",
  name: "Twisted Ribbon",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 6,
    nextPartNum: 2,
  },
};
