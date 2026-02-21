import type { Example } from "./index";
import type { Document, SketchSegment2D } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Vase shape using Loft between circular profiles at different heights
// Z is UP - profiles are on XY planes at different Z positions
//
// LEARNING POINTS:
// - Loft operation: interpolates surface between multiple sketch profiles
// - Multiple Sketch2D nodes at different origins (heights)
// - Profile radii control the shape silhouette
// - closed: false lofts from first to last profile (default)
// - closed: true would connect last profile back to first (for tubular loops)

// Helper to create a polygon approximating a circle on the XZ plane
function createCircleProfile(radius: number, segments: number): SketchSegment2D[] {
  const result: SketchSegment2D[] = [];
  for (let i = 0; i < segments; i++) {
    const angle1 = (i / segments) * 2 * Math.PI;
    const angle2 = ((i + 1) / segments) * 2 * Math.PI;
    result.push({
      type: "Line",
      start: { x: Math.cos(angle1) * radius, y: Math.sin(angle1) * radius },
      end: { x: Math.cos(angle2) * radius, y: Math.sin(angle2) * radius },
    });
  }
  return result;
}

// Profile radii create classic vase silhouette:
//   Bottom (z=0):  15mm - small base
//   Bulge (z=30):  35mm - widest point
//   Neck (z=70):   12mm - narrow waist
//   Lip (z=90):    18mm - flared opening
const SEGMENTS = 24;

const document: Document = {
  version: "0.1",
  nodes: {
    // === LOFT PROFILES (horizontal slices at different heights) ===
    // Profile 1: Base (z=0, r=15mm)
    "1": {
      id: 1,
      name: "Profile 1: Base (r=15)",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 0 },
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 1, z: 0 },
        segments: createCircleProfile(15, SEGMENTS),
      },
    },
    // Profile 2: Bulge (z=30, r=35mm)
    "2": {
      id: 2,
      name: "Profile 2: Bulge (r=35)",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 30 },
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 1, z: 0 },
        segments: createCircleProfile(35, SEGMENTS),
      },
    },
    // Profile 3: Neck (z=70, r=12mm)
    "3": {
      id: 3,
      name: "Profile 3: Neck (r=12)",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 70 },
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 1, z: 0 },
        segments: createCircleProfile(12, SEGMENTS),
      },
    },
    // Profile 4: Lip (z=90, r=18mm)
    "4": {
      id: 4,
      name: "Profile 4: Lip (r=18)",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 90 },
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 1, z: 0 },
        segments: createCircleProfile(18, SEGMENTS),
      },
    },

    // === LOFT OPERATION ===
    // Interpolate smooth surface between all 4 profiles
    "5": {
      id: 5,
      name: "Loft (4 profiles)",
      op: {
        type: "Loft",
        sketches: [1, 2, 3, 4],  // Bottom to top
        closed: false,          // Open solid (not a tube)
      },
    },

    // === FINAL TRANSFORMS ===
    "6": { id: 6, name: "Scale", op: { type: "Scale", child: 5, factor: { x: 1, y: 1, z: 1 } } },
    "7": { id: 7, name: "Rotate", op: { type: "Rotate", child: 6, angles: { x: 0, y: 0, z: 0 } } },
    "8": { id: 8, name: "Vase", op: { type: "Translate", child: 7, offset: { x: 0, y: 0, z: 0 } } },
  },
  materials: {
    terracotta: {
      name: "Terracotta",
      color: [0.76, 0.38, 0.28],
      metallic: 0.0,
      roughness: 0.85,
    },
  },
  part_materials: {},
  roots: [{ root: 8, material: "terracotta" }],
  // === ASSEMBLY STRUCTURE ===
  partDefs: {
    vase: {
      id: "vase",
      name: "Vase",
      root: 8,
      defaultMaterial: "terracotta",
    },
  },
  instances: [
    {
      id: "vase-1",
      partDefId: "vase",
      name: "Vase",
      transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } },
    },
  ],
  groundInstanceId: "vase-1",
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Vase",
    kind: "loft",
    sketchNodeIds: [1, 2, 3, 4],
    loftNodeId: 5,
    scaleNodeId: 6,
    rotateNodeId: 7,
    translateNodeId: 8,
  },
];

export const vaseExample: Example = {
  id: "vase",
  name: "Vase",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 9,
    nextPartNum: 2,
  },
};
