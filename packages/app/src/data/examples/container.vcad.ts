import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Container box - demonstrates Shell operation to create hollow geometry
// A simple storage container with 3mm walls and a handle on top
// Z is UP
//
// LEARNING POINTS:
// - Shell operation: hollows a solid leaving walls of specified thickness
// - Multi-part assembly: container body + handle components
// - Difference operation to create ring (cylinder - smaller cylinder)
// - Non-uniform scale to create elliptical cross-section
const document: Document = {
  version: "0.1",
  nodes: {
    // === CONTAINER BODY ===
    // Base cube: 40x30x25mm (before shell)
    "1": { id: 1, name: "Container Solid", op: { type: "Cube", size: { x: 40, y: 30, z: 25 } } },
    // Shell operation: hollows from top face, 3mm wall thickness
    "2": { id: 2, name: "Shell (3mm walls)", op: { type: "Shell", child: 1, thickness: 3 } },
    "3": { id: 3, name: "Container Scale", op: { type: "Scale", child: 2, factor: { x: 1, y: 1, z: 1 } } },
    "4": { id: 4, name: "Container Rotate", op: { type: "Rotate", child: 3, angles: { x: 0, y: 0, z: 0 } } },
    "5": { id: 5, name: "Container Body", op: { type: "Translate", child: 4, offset: { x: 0, y: 0, z: 0 } } },

    // === HANDLE BAR ===
    // Horizontal cylinder spanning container width
    "6": { id: 6, name: "Handle Bar Cylinder", op: { type: "Cylinder", radius: 2, height: 24, segments: 16 } },
    "7": { id: 7, name: "Handle Bar Scale", op: { type: "Scale", child: 6, factor: { x: 1, y: 1, z: 1 } } },
    "8": { id: 8, name: "Handle Bar Rotate (X=-90°)", op: { type: "Rotate", child: 7, angles: { x: -90, y: 0, z: 0 } } },
    "9": { id: 9, name: "Handle Bar", op: { type: "Translate", child: 8, offset: { x: 0, y: 0, z: 32 } } },

    // === HANDLE SUPPORTS ===
    // Left support: vertical post connecting bar to container
    "10": { id: 10, name: "Left Support Cylinder", op: { type: "Cylinder", radius: 2, height: 7, segments: 16 } },
    "11": { id: 11, name: "Left Support Scale", op: { type: "Scale", child: 10, factor: { x: 1, y: 1, z: 1 } } },
    "12": { id: 12, name: "Left Support Rotate", op: { type: "Rotate", child: 11, angles: { x: 0, y: 0, z: 0 } } },
    "13": { id: 13, name: "Left Support", op: { type: "Translate", child: 12, offset: { x: -12, y: 0, z: 25 } } },

    // Right support: vertical post connecting bar to container
    "14": { id: 14, name: "Right Support Cylinder", op: { type: "Cylinder", radius: 2, height: 7, segments: 16 } },
    "15": { id: 15, name: "Right Support Scale", op: { type: "Scale", child: 14, factor: { x: 1, y: 1, z: 1 } } },
    "16": { id: 16, name: "Right Support Rotate", op: { type: "Rotate", child: 15, angles: { x: 0, y: 0, z: 0 } } },
    "17": { id: 17, name: "Right Support", op: { type: "Translate", child: 16, offset: { x: 12, y: 0, z: 25 } } },

    // === DECORATIVE RIM ===
    // Ring created by subtracting smaller cylinder from larger
    "18": { id: 18, name: "Rim Outer Cylinder", op: { type: "Cylinder", radius: 22, height: 2, segments: 32 } },
    "19": { id: 19, name: "Rim Inner Cylinder", op: { type: "Cylinder", radius: 19, height: 2.5, segments: 32 } },
    "20": { id: 20, name: "Rim Ring (Difference)", op: { type: "Difference", left: 18, right: 19 } },
    // Non-uniform scale creates elliptical shape to match container
    "21": { id: 21, name: "Rim Scale (elliptical)", op: { type: "Scale", child: 20, factor: { x: 0.91, y: 0.68, z: 1 } } },
    "22": { id: 22, name: "Rim Rotate", op: { type: "Rotate", child: 21, angles: { x: 0, y: 0, z: 0 } } },
    "23": { id: 23, name: "Rim", op: { type: "Translate", child: 22, offset: { x: 0, y: 0, z: 24 } } },
  },
  materials: {
    container: {
      name: "Container",
      color: [0.2, 0.6, 0.85],
      metallic: 0.0,
      roughness: 0.4,
    },
    handle: {
      name: "Handle",
      color: [0.3, 0.3, 0.35],
      metallic: 0.1,
      roughness: 0.5,
    },
    rim: {
      name: "Rim",
      color: [0.25, 0.55, 0.8],
      metallic: 0.0,
      roughness: 0.35,
    },
  },
  part_materials: {},
  roots: [
    { root: 5, material: "container" },
    { root: 9, material: "handle" },
    { root: 13, material: "handle" },
    { root: 17, material: "handle" },
    { root: 23, material: "rim" },
  ],
  // === ASSEMBLY STRUCTURE ===
  // Multi-part assembly: body, handle components, and rim
  partDefs: {
    body: { id: "body", name: "Container Body", root: 5, defaultMaterial: "container" },
    handleBar: { id: "handleBar", name: "Handle Bar", root: 9, defaultMaterial: "handle" },
    leftSupport: { id: "leftSupport", name: "Left Support", root: 13, defaultMaterial: "handle" },
    rightSupport: { id: "rightSupport", name: "Right Support", root: 17, defaultMaterial: "handle" },
    rim: { id: "rim", name: "Rim", root: 23, defaultMaterial: "rim" },
  },
  instances: [
    { id: "body-1", partDefId: "body", name: "Container Body", transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } } },
    { id: "handleBar-1", partDefId: "handleBar", name: "Handle Bar", transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } } },
    { id: "leftSupport-1", partDefId: "leftSupport", name: "Left Support", transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } } },
    { id: "rightSupport-1", partDefId: "rightSupport", name: "Right Support", transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } } },
    { id: "rim-1", partDefId: "rim", name: "Rim", transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } } },
  ],
  groundInstanceId: "body-1",
};

const parts: PartInfo[] = [
  // The container uses Shell internally, but we track it as a cube primitive
  // since that's what the user would interact with
  { id: "part-1", name: "Container", kind: "cube", primitiveNodeId: 1, scaleNodeId: 3, rotateNodeId: 4, translateNodeId: 5 },
  { id: "part-2", name: "Handle Bar", kind: "cylinder", primitiveNodeId: 6, scaleNodeId: 7, rotateNodeId: 8, translateNodeId: 9 },
  { id: "part-3", name: "Left Support", kind: "cylinder", primitiveNodeId: 10, scaleNodeId: 11, rotateNodeId: 12, translateNodeId: 13 },
  { id: "part-4", name: "Right Support", kind: "cylinder", primitiveNodeId: 14, scaleNodeId: 15, rotateNodeId: 16, translateNodeId: 17 },
];

export const containerExample: Example = {
  id: "container",
  name: "Container",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 24,
    nextPartNum: 5,
  },
};
