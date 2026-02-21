import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Flange plate with bolt holes arranged in a circular pattern
// Demonstrates CircularPattern operation for manufacturing applications
//
// LEARNING POINTS:
// - Cylinder primitive for circular plate base
// - CircularPattern to replicate a single hole around an axis
// - Efficient way to create symmetric hole patterns
// - Nested boolean operations (center hole, then bolt pattern)
const document: Document = {
  version: "0.1",
  nodes: {
    // === BASE PLATE ===
    // Circular plate: radius 50mm, height 10mm
    "1": { id: 1, name: "Plate Cylinder", op: { type: "Cylinder", radius: 50, height: 10, segments: 64 } },

    // === CENTER BORE ===
    // Center bore: radius 15mm (for shaft clearance)
    "2": { id: 2, name: "Center Bore Cylinder", op: { type: "Cylinder", radius: 15, height: 20, segments: 32 } },
    // Position to fully cut through plate
    "3": { id: 3, name: "Center Bore Positioned", op: { type: "Translate", child: 2, offset: { x: 0, y: -5, z: 0 } } },

    // Subtract center hole from plate
    "4": { id: 4, name: "Plate with Center Bore", op: { type: "Difference", left: 1, right: 3 } },

    // === BOLT HOLE PATTERN ===
    // Single bolt hole template: radius 4mm
    "10": { id: 10, name: "Bolt Hole Template", op: { type: "Cylinder", radius: 4, height: 20, segments: 24 } },
    // Position at bolt circle radius (35mm from center)
    "11": { id: 11, name: "Bolt Hole at Radius", op: { type: "Translate", child: 10, offset: { x: 35, y: -5, z: 0 } } },

    // CircularPattern: replicate bolt hole 6 times around Z axis
    "12": {
      id: 12,
      name: "Bolt Hole Pattern",
      op: {
        type: "CircularPattern",
        child: 11,
        axis_origin: { x: 0, y: 0, z: 0 },
        axis_dir: { x: 0, y: 0, z: 1 },  // Z-up rotation axis
        count: 6,                         // 6 holes
        angle_deg: 360,                   // Full circle (60° between holes)
      },
    },

    // Subtract all bolt holes from plate
    "20": { id: 20, name: "Flange with All Holes", op: { type: "Difference", left: 4, right: 12 } },

    // === FINAL TRANSFORMS ===
    "30": { id: 30, name: "Scale", op: { type: "Scale", child: 20, factor: { x: 1, y: 1, z: 1 } } },
    "31": { id: 31, name: "Rotate", op: { type: "Rotate", child: 30, angles: { x: 0, y: 0, z: 0 } } },
    "32": { id: 32, name: "Flange Plate", op: { type: "Translate", child: 31, offset: { x: 0, y: 0, z: 0 } } },
  },
  materials: {
    steel: {
      name: "Steel",
      color: [0.6, 0.6, 0.65],
      metallic: 0.9,
      roughness: 0.4,
    },
  },
  part_materials: {},
  roots: [{ root: 32, material: "steel" }],
  // === ASSEMBLY STRUCTURE ===
  partDefs: {
    flange: {
      id: "flange",
      name: "Flange Plate",
      root: 32,
      defaultMaterial: "steel",
    },
  },
  instances: [
    {
      id: "flange-1",
      partDefId: "flange",
      name: "Flange Plate",
      transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } },
    },
  ],
  groundInstanceId: "flange-1",
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Flange Plate",
    kind: "cylinder",
    primitiveNodeId: 1,
    scaleNodeId: 30,
    rotateNodeId: 31,
    translateNodeId: 32,
  },
];

export const flangeExample: Example = {
  id: "flange",
  name: "Flange",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 33,
    nextPartNum: 2,
  },
};
