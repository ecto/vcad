import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// 2-DOF Robot Arm with joints for physics simulation (Z-up)
// Articulated arm with horizontal joint axes - falls under gravity
// Parts are defined at origin, FK positions them via joint chain
const document: Document = {
  version: "0.1",
  nodes: {
    // Base pedestal - centered at origin
    "1": {
      id: 1,
      name: "Base",
      op: { type: "Cylinder", radius: 40, height: 30, segments: 32 },
    },
    // Upper arm - horizontal beam
    "2": {
      id: 2,
      name: "Upper Arm",
      op: { type: "Cube", size: { x: 80, y: 20, z: 20 } },
    },
    // Lower arm - horizontal beam
    "3": {
      id: 3,
      name: "Lower Arm",
      op: { type: "Cube", size: { x: 60, y: 16, z: 16 } },
    },
    // Gripper - small cylinder
    "4": {
      id: 4,
      name: "Gripper",
      op: { type: "Cylinder", radius: 8, height: 25, segments: 32 },
    },
  },
  materials: {
    steel: {
      name: "Steel",
      color: [0.45, 0.45, 0.5],
      metallic: 1.0,
      roughness: 0.3,
      density: 7850,
      friction: 0.5,
    },
    aluminum: {
      name: "Aluminum",
      color: [0.85, 0.87, 0.9],
      metallic: 1.0,
      roughness: 0.4,
      density: 2700,
      friction: 0.6,
    },
    orange: {
      name: "Orange Plastic",
      color: [1.0, 0.5, 0.1],
      metallic: 0.0,
      roughness: 0.5,
      density: 1200,
      friction: 0.4,
    },
  },
  part_materials: {
    base: "steel",
    upper_arm: "aluminum",
    lower_arm: "aluminum",
    gripper: "orange",
  },
  partDefs: {
    base: {
      id: "base",
      name: "Base",
      root: 1,
      defaultMaterial: "steel",
    },
    upper_arm: {
      id: "upper_arm",
      name: "Upper Arm",
      root: 2,
      defaultMaterial: "aluminum",
    },
    lower_arm: {
      id: "lower_arm",
      name: "Lower Arm",
      root: 3,
      defaultMaterial: "aluminum",
    },
    gripper: {
      id: "gripper",
      name: "Gripper",
      root: 4,
      defaultMaterial: "orange",
    },
  },
  instances: [
    {
      id: "base_inst",
      partDefId: "base",
      name: "Base",
      // Base centered at origin - only ground instance has explicit transform
      transform: {
        translation: { x: -40, y: -40, z: 0 },
        rotation: { x: 0, y: 0, z: 0 },
        scale: { x: 1, y: 1, z: 1 },
      },
    },
    {
      id: "upper_arm_inst",
      partDefId: "upper_arm",
      name: "Upper Arm",
      // FK determines position via joint chain - no explicit transform
    },
    {
      id: "lower_arm_inst",
      partDefId: "lower_arm",
      name: "Lower Arm",
      // FK determines position via joint chain - no explicit transform
    },
    {
      id: "gripper_inst",
      partDefId: "gripper",
      name: "Gripper",
      // FK determines position via joint chain - no explicit transform
    },
  ],
  joints: [
    {
      id: "shoulder",
      name: "Shoulder Joint",
      parentInstanceId: "base_inst",
      childInstanceId: "upper_arm_inst",
      // Joint at top center of base
      parentAnchor: { x: 40, y: 40, z: 30 },
      // Connects to back-center of upper arm
      childAnchor: { x: 0, y: 10, z: 10 },
      kind: {
        type: "Revolute",
        // Horizontal axis (Y) - gravity creates torque on this joint
        axis: { x: 0, y: 1, z: 0 },
        limits: [-120, 120],
      },
      state: 0,
    },
    {
      id: "elbow",
      name: "Elbow Joint",
      parentInstanceId: "upper_arm_inst",
      childInstanceId: "lower_arm_inst",
      // Joint at end of upper arm
      parentAnchor: { x: 80, y: 10, z: 10 },
      // Connects to back of lower arm
      childAnchor: { x: 0, y: 8, z: 8 },
      kind: {
        type: "Revolute",
        // Horizontal axis (Y) - gravity creates torque on this joint
        axis: { x: 0, y: 1, z: 0 },
        limits: [-135, 135],
      },
      state: 0,
    },
    {
      id: "wrist",
      name: "Wrist Joint",
      parentInstanceId: "lower_arm_inst",
      childInstanceId: "gripper_inst",
      // Joint at end of lower arm
      parentAnchor: { x: 60, y: 8, z: 8 },
      // Connects to top of gripper
      childAnchor: { x: 8, y: 8, z: 25 },
      kind: { type: "Fixed" },
      state: 0,
    },
  ],
  groundInstanceId: "base_inst",
  // Assembly mode: geometry comes from partDefs → instances, not roots
  roots: [],
};

// Assembly mode: parts are rendered via instances, not traditional PartInfo
const parts: PartInfo[] = [];

export const robotArmExample: Example = {
  id: "robot-arm",
  name: "Robot Arm",
  description: "SCARA-style robot arm with shoulder and elbow joints",
  difficulty: "intermediate",
  features: ["assembly", "joints", "kinematics"],
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 5,
    nextPartNum: 5,
  },
};
