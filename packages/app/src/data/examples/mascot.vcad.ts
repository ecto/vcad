import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// V-Bot mascot - cute robot with chibi proportions
// Z is UP (standard CAD convention)
// Design: Big round head, small body, stubby limbs
const document: Document = {
  version: "0.1",
  nodes: {
    // === HEAD (big and round - chibi style) ===
    "1": { id: 1, name: null, op: { type: "Sphere", radius: 14, segments: 32 } },
    "2": { id: 2, name: null, op: { type: "Scale", child: 1, factor: { x: 1, y: 1, z: 1 } } },
    "3": { id: 3, name: null, op: { type: "Rotate", child: 2, angles: { x: 0, y: 0, z: 0 } } },
    "4": { id: 4, name: "Head", op: { type: "Translate", child: 3, offset: { x: 0, y: 0, z: 32 } } },

    // === BODY (smaller, pill-shaped) ===
    "5": { id: 5, name: null, op: { type: "Sphere", radius: 10, segments: 24 } },
    "6": { id: 6, name: null, op: { type: "Scale", child: 5, factor: { x: 1, y: 0.9, z: 1.3 } } },
    "7": { id: 7, name: null, op: { type: "Rotate", child: 6, angles: { x: 0, y: 0, z: 0 } } },
    "8": { id: 8, name: "Body", op: { type: "Translate", child: 7, offset: { x: 0, y: 0, z: 12 } } },

    // === EYES (big, cute, slightly apart) ===
    // Left eye white
    "9": { id: 9, name: null, op: { type: "Sphere", radius: 4.5, segments: 20 } },
    "10": { id: 10, name: null, op: { type: "Scale", child: 9, factor: { x: 1, y: 1, z: 1.1 } } },
    "11": { id: 11, name: null, op: { type: "Rotate", child: 10, angles: { x: 0, y: 0, z: 0 } } },
    "12": { id: 12, name: "Left Eye", op: { type: "Translate", child: 11, offset: { x: -5, y: 11, z: 34 } } },

    // Right eye white
    "13": { id: 13, name: null, op: { type: "Sphere", radius: 4.5, segments: 20 } },
    "14": { id: 14, name: null, op: { type: "Scale", child: 13, factor: { x: 1, y: 1, z: 1.1 } } },
    "15": { id: 15, name: null, op: { type: "Rotate", child: 14, angles: { x: 0, y: 0, z: 0 } } },
    "16": { id: 16, name: "Right Eye", op: { type: "Translate", child: 15, offset: { x: 5, y: 11, z: 34 } } },

    // Left pupil (centered on eye, looking forward)
    "17": { id: 17, name: null, op: { type: "Sphere", radius: 2.2, segments: 16 } },
    "18": { id: 18, name: null, op: { type: "Scale", child: 17, factor: { x: 1, y: 1, z: 1 } } },
    "19": { id: 19, name: null, op: { type: "Rotate", child: 18, angles: { x: 0, y: 0, z: 0 } } },
    "20": { id: 20, name: "Left Pupil", op: { type: "Translate", child: 19, offset: { x: -5, y: 14.5, z: 35 } } },

    // Right pupil (centered on eye, looking forward)
    "21": { id: 21, name: null, op: { type: "Sphere", radius: 2.2, segments: 16 } },
    "22": { id: 22, name: null, op: { type: "Scale", child: 21, factor: { x: 1, y: 1, z: 1 } } },
    "23": { id: 23, name: null, op: { type: "Rotate", child: 22, angles: { x: 0, y: 0, z: 0 } } },
    "24": { id: 24, name: "Right Pupil", op: { type: "Translate", child: 23, offset: { x: 5, y: 14.5, z: 35 } } },

    // === CHEEKS (rosy blush - small flat ovals on face) ===
    "33": { id: 33, name: null, op: { type: "Sphere", radius: 1.8, segments: 12 } },
    "34": { id: 34, name: null, op: { type: "Scale", child: 33, factor: { x: 1.2, y: 0.3, z: 0.7 } } },
    "35": { id: 35, name: null, op: { type: "Rotate", child: 34, angles: { x: 0, y: 0, z: 0 } } },
    "36": { id: 36, name: "Left Cheek", op: { type: "Translate", child: 35, offset: { x: -9, y: 13, z: 30 } } },

    "37": { id: 37, name: null, op: { type: "Sphere", radius: 1.8, segments: 12 } },
    "38": { id: 38, name: null, op: { type: "Scale", child: 37, factor: { x: 1.2, y: 0.3, z: 0.7 } } },
    "39": { id: 39, name: null, op: { type: "Rotate", child: 38, angles: { x: 0, y: 0, z: 0 } } },
    "40": { id: 40, name: "Right Cheek", op: { type: "Translate", child: 39, offset: { x: 9, y: 13, z: 30 } } },

    // === ANTENNA ===
    // Stem
    "41": { id: 41, name: null, op: { type: "Cylinder", radius: 1, height: 8, segments: 12 } },
    "42": { id: 42, name: null, op: { type: "Scale", child: 41, factor: { x: 1, y: 1, z: 1 } } },
    "43": { id: 43, name: null, op: { type: "Rotate", child: 42, angles: { x: 0, y: 0, z: 0 } } },
    "44": { id: 44, name: "Antenna Stem", op: { type: "Translate", child: 43, offset: { x: 0, y: 0, z: 46 } } },

    // Tip ball
    "45": { id: 45, name: null, op: { type: "Sphere", radius: 2.5, segments: 16 } },
    "46": { id: 46, name: null, op: { type: "Scale", child: 45, factor: { x: 1, y: 1, z: 1 } } },
    "47": { id: 47, name: null, op: { type: "Rotate", child: 46, angles: { x: 0, y: 0, z: 0 } } },
    "48": { id: 48, name: "Antenna Tip", op: { type: "Translate", child: 47, offset: { x: 0, y: 0, z: 56 } } },

    // === HANDS (ball hands attached to body - snowman style) ===
    // Left hand - touching body side
    "49": { id: 49, name: null, op: { type: "Sphere", radius: 4, segments: 16 } },
    "50": { id: 50, name: null, op: { type: "Scale", child: 49, factor: { x: 1, y: 1, z: 1 } } },
    "51": { id: 51, name: null, op: { type: "Rotate", child: 50, angles: { x: 0, y: 0, z: 0 } } },
    "52": { id: 52, name: "Left Hand", op: { type: "Translate", child: 51, offset: { x: -13, y: 0, z: 12 } } },

    // Right hand - touching body side
    "53": { id: 53, name: null, op: { type: "Sphere", radius: 4, segments: 16 } },
    "54": { id: 54, name: null, op: { type: "Scale", child: 53, factor: { x: 1, y: 1, z: 1 } } },
    "55": { id: 55, name: null, op: { type: "Rotate", child: 54, angles: { x: 0, y: 0, z: 0 } } },
    "56": { id: 56, name: "Right Hand", op: { type: "Translate", child: 55, offset: { x: 13, y: 0, z: 12 } } },

    // === FEET (stubby cylinders) ===
    "57": { id: 57, name: null, op: { type: "Cylinder", radius: 4, height: 3, segments: 16 } },
    "58": { id: 58, name: null, op: { type: "Scale", child: 57, factor: { x: 1, y: 1.2, z: 1 } } },
    "59": { id: 59, name: null, op: { type: "Rotate", child: 58, angles: { x: 0, y: 0, z: 0 } } },
    "60": { id: 60, name: "Left Foot", op: { type: "Translate", child: 59, offset: { x: -5, y: 0, z: 0 } } },

    "61": { id: 61, name: null, op: { type: "Cylinder", radius: 4, height: 3, segments: 16 } },
    "62": { id: 62, name: null, op: { type: "Scale", child: 61, factor: { x: 1, y: 1.2, z: 1 } } },
    "63": { id: 63, name: null, op: { type: "Rotate", child: 62, angles: { x: 0, y: 0, z: 0 } } },
    "64": { id: 64, name: "Right Foot", op: { type: "Translate", child: 63, offset: { x: 5, y: 0, z: 0 } } },

    // === BELLY BUTTON (cute detail on body surface) ===
    "65": { id: 65, name: null, op: { type: "Sphere", radius: 1.2, segments: 12 } },
    "66": { id: 66, name: null, op: { type: "Scale", child: 65, factor: { x: 1, y: 1, z: 1 } } },
    "67": { id: 67, name: null, op: { type: "Rotate", child: 66, angles: { x: 0, y: 0, z: 0 } } },
    "68": { id: 68, name: "Belly Button", op: { type: "Translate", child: 67, offset: { x: 0, y: 9.5, z: 8 } } },
  },
  materials: {
    head: {
      name: "Head",
      color: [0.45, 0.78, 0.95],  // Sky blue
      metallic: 0.1,
      roughness: 0.4,
    },
    body: {
      name: "Body",
      color: [0.4, 0.72, 0.9],  // Slightly darker blue
      metallic: 0.1,
      roughness: 0.4,
    },
    eye_white: {
      name: "Eye White",
      color: [1.0, 1.0, 1.0],
      metallic: 0.0,
      roughness: 0.15,
    },
    pupil: {
      name: "Pupil",
      color: [0.15, 0.15, 0.2],
      metallic: 0.0,
      roughness: 0.1,
    },
    cheek: {
      name: "Cheek",
      color: [0.95, 0.55, 0.6],  // Rosy pink
      metallic: 0.0,
      roughness: 0.7,
    },
    antenna: {
      name: "Antenna",
      color: [0.95, 0.7, 0.2],  // Golden yellow
      metallic: 0.5,
      roughness: 0.3,
    },
    limb: {
      name: "Limb",
      color: [0.92, 0.92, 0.95],  // Off-white
      metallic: 0.0,
      roughness: 0.4,
    },
    foot: {
      name: "Foot",
      color: [0.35, 0.4, 0.5],  // Dark grey-blue
      metallic: 0.2,
      roughness: 0.5,
    },
    accent: {
      name: "Accent",
      color: [0.95, 0.5, 0.4],  // Coral accent
      metallic: 0.2,
      roughness: 0.4,
    },
  },
  part_materials: {},
  roots: [
    // Head & Body
    { root: 4, material: "head" },
    { root: 8, material: "body" },
    // Eyes
    { root: 12, material: "eye_white" },
    { root: 16, material: "eye_white" },
    { root: 20, material: "pupil" },
    { root: 24, material: "pupil" },
    // Cheeks
    { root: 36, material: "cheek" },
    { root: 40, material: "cheek" },
    // Antenna
    { root: 44, material: "limb" },
    { root: 48, material: "antenna" },
    // Hands
    { root: 52, material: "limb" },
    { root: 56, material: "limb" },
    // Feet
    { root: 60, material: "foot" },
    { root: 64, material: "foot" },
    // Belly button
    { root: 68, material: "antenna" },
  ],
};

const parts: PartInfo[] = [
  { id: "part-1", name: "Head", kind: "sphere", primitiveNodeId: 1, scaleNodeId: 2, rotateNodeId: 3, translateNodeId: 4 },
  { id: "part-2", name: "Body", kind: "sphere", primitiveNodeId: 5, scaleNodeId: 6, rotateNodeId: 7, translateNodeId: 8 },
  { id: "part-3", name: "Left Eye", kind: "sphere", primitiveNodeId: 9, scaleNodeId: 10, rotateNodeId: 11, translateNodeId: 12 },
  { id: "part-4", name: "Right Eye", kind: "sphere", primitiveNodeId: 13, scaleNodeId: 14, rotateNodeId: 15, translateNodeId: 16 },
  { id: "part-5", name: "Left Pupil", kind: "sphere", primitiveNodeId: 17, scaleNodeId: 18, rotateNodeId: 19, translateNodeId: 20 },
  { id: "part-6", name: "Right Pupil", kind: "sphere", primitiveNodeId: 21, scaleNodeId: 22, rotateNodeId: 23, translateNodeId: 24 },
  { id: "part-7", name: "Left Cheek", kind: "sphere", primitiveNodeId: 33, scaleNodeId: 34, rotateNodeId: 35, translateNodeId: 36 },
  { id: "part-8", name: "Right Cheek", kind: "sphere", primitiveNodeId: 37, scaleNodeId: 38, rotateNodeId: 39, translateNodeId: 40 },
  { id: "part-9", name: "Antenna Stem", kind: "cylinder", primitiveNodeId: 41, scaleNodeId: 42, rotateNodeId: 43, translateNodeId: 44 },
  { id: "part-10", name: "Antenna Tip", kind: "sphere", primitiveNodeId: 45, scaleNodeId: 46, rotateNodeId: 47, translateNodeId: 48 },
  { id: "part-11", name: "Left Hand", kind: "sphere", primitiveNodeId: 49, scaleNodeId: 50, rotateNodeId: 51, translateNodeId: 52 },
  { id: "part-12", name: "Right Hand", kind: "sphere", primitiveNodeId: 53, scaleNodeId: 54, rotateNodeId: 55, translateNodeId: 56 },
  { id: "part-13", name: "Left Foot", kind: "cylinder", primitiveNodeId: 57, scaleNodeId: 58, rotateNodeId: 59, translateNodeId: 60 },
  { id: "part-14", name: "Right Foot", kind: "cylinder", primitiveNodeId: 61, scaleNodeId: 62, rotateNodeId: 63, translateNodeId: 64 },
  { id: "part-15", name: "Belly Button", kind: "sphere", primitiveNodeId: 65, scaleNodeId: 66, rotateNodeId: 67, translateNodeId: 68 },
];

export const mascotExample: Example = {
  id: "mascot",
  name: "Mascot",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 69,
    nextPartNum: 16,
  },
};
