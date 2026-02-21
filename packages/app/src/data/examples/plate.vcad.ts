import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Mounting plate with center hole and mounting holes (circular)
// Demonstrates boolean difference operations with rotated cylinders
const document: Document = {
  version: "0.1",
  nodes: {
    // Plate primitive: 80x60x6
    "1": {
      id: 1,
      name: null,
      op: { type: "Cube", size: { x: 80, y: 60, z: 6 } },
    },

    // Large center hole (cylinder, radius 6, already along Z axis)
    "2": {
      id: 2,
      name: null,
      op: { type: "Cylinder", radius: 6, height: 20, segments: 32 },
    },
    // No rotation needed — cylinder already along Z axis
    "3": {
      id: 3,
      name: null,
      op: { type: "Rotate", child: 2, angles: { x: 0, y: 0, z: 0 } },
    },
    // Position at center of plate
    "4": {
      id: 4,
      name: null,
      op: { type: "Translate", child: 3, offset: { x: 40, y: 30, z: -7 } },
    },

    // Small mounting holes (cylinder, radius 2, already along Z axis)
    // Hole template
    "10": {
      id: 10,
      name: null,
      op: { type: "Cylinder", radius: 2, height: 20, segments: 24 },
    },
    // No rotation needed — cylinder already along Z axis
    "11": {
      id: 11,
      name: null,
      op: { type: "Rotate", child: 10, angles: { x: 0, y: 0, z: 0 } },
    },

    // Corner holes - positioned at corners with 8mm inset from edge
    // Front-left (8, 8, z)
    "20": {
      id: 20,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 8, y: 8, z: -7 } },
    },
    // Front-right (72, 8, z)
    "21": {
      id: 21,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 72, y: 8, z: -7 } },
    },
    // Back-left (8, 52, z)
    "22": {
      id: 22,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 8, y: 52, z: -7 } },
    },
    // Back-right (72, 52, z)
    "23": {
      id: 23,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 72, y: 52, z: -7 } },
    },

    // Edge holes - positioned at edge midpoints
    // Left edge (8, 30, z)
    "24": {
      id: 24,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 8, y: 30, z: -7 } },
    },
    // Right edge (72, 30, z)
    "25": {
      id: 25,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 72, y: 30, z: -7 } },
    },
    // Front edge (40, 8, z)
    "26": {
      id: 26,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 40, y: 8, z: -7 } },
    },
    // Back edge (40, 52, z)
    "27": {
      id: 27,
      name: null,
      op: { type: "Translate", child: 11, offset: { x: 40, y: 52, z: -7 } },
    },

    // Union all holes together
    "30": { id: 30, name: null, op: { type: "Union", left: 4, right: 20 } },
    "31": { id: 31, name: null, op: { type: "Union", left: 30, right: 21 } },
    "32": { id: 32, name: null, op: { type: "Union", left: 31, right: 22 } },
    "33": { id: 33, name: null, op: { type: "Union", left: 32, right: 23 } },
    "34": { id: 34, name: null, op: { type: "Union", left: 33, right: 24 } },
    "35": { id: 35, name: null, op: { type: "Union", left: 34, right: 25 } },
    "36": { id: 36, name: null, op: { type: "Union", left: 35, right: 26 } },
    "37": { id: 37, name: null, op: { type: "Union", left: 36, right: 27 } },

    // Boolean difference: plate - all holes
    "40": {
      id: 40,
      name: null,
      op: { type: "Difference", left: 1, right: 37 },
    },

    // Transform the result to center it
    "50": {
      id: 50,
      name: null,
      op: { type: "Scale", child: 40, factor: { x: 1, y: 1, z: 1 } },
    },
    "51": {
      id: 51,
      name: null,
      op: { type: "Rotate", child: 50, angles: { x: 0, y: 0, z: 0 } },
    },
    "52": {
      id: 52,
      name: "Mounting Plate",
      op: { type: "Translate", child: 51, offset: { x: -40, y: -30, z: 0 } },
    },
  },
  materials: {
    default: {
      name: "Default",
      color: [0.55, 0.55, 0.55],
      metallic: 0.0,
      roughness: 0.7,
    },
  },
  part_materials: {},
  roots: [{ root: 52, material: "default" }],
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Mounting Plate",
    kind: "cube",
    primitiveNodeId: 1,
    scaleNodeId: 50,
    rotateNodeId: 51,
    translateNodeId: 52,
  },
];

export const plateExample: Example = {
  id: "plate",
  name: "Mounting Plate",
  description:
    "A plate with a center hole and mounting holes. Demonstrates boolean difference operations.",
  difficulty: "beginner",
  thumbnail: "/assets/plate.png",
  features: ["Primitives", "Transforms", "Boolean Difference"],
  unlockAfter: 0,
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 55,
    nextPartNum: 2,
  },
};
