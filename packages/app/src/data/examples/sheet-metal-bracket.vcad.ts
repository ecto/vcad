import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Sheet-metal U-channel — base flange (100×50, 1mm aluminum) with two
// 25mm edge flanges along the long edges. Demonstrates the foundation
// tier of `@vcad/sheet-metal`: a panel/bend graph with lossless unfold,
// rendered both as a 3D mesh and a flat pattern.
const document: Document = {
  version: "0.1",
  nodes: {
    // Base flange (1mm aluminum, 100mm × 50mm)
    "1": {
      id: 1,
      name: "Base flange",
      op: {
        type: "SheetMetalBaseFlangeRect",
        width: 100,
        depth: 50,
        thickness: 1.0,
        material: "Al-soft",
      },
    },
    // First edge flange — Up off edge 0 (y=0 long edge)
    "2": {
      id: 2,
      name: "Front flange",
      op: {
        type: "SheetMetalEdgeFlange",
        parent: 1,
        panel_id: 0,
        edge_index: 0,
        length: 25,
        angle: Math.PI / 2,
        radius: 1.0,
        direction: "Up",
      },
    },
    // Second edge flange — Up off edge 2 (y=50 long edge)
    "3": {
      id: 3,
      name: "Back flange",
      op: {
        type: "SheetMetalEdgeFlange",
        parent: 2,
        panel_id: 0,
        edge_index: 2,
        length: 25,
        angle: Math.PI / 2,
        radius: 1.0,
        direction: "Up",
      },
    },
    // Standard scale/rotate/translate chain to satisfy PartInfo
    "10": {
      id: 10,
      name: null,
      op: { type: "Scale", child: 3, factor: { x: 1, y: 1, z: 1 } },
    },
    "11": {
      id: 11,
      name: null,
      op: { type: "Rotate", child: 10, angles: { x: 0, y: 0, z: 0 } },
    },
    "12": {
      id: 12,
      name: "Sheet metal U-channel",
      op: { type: "Translate", child: 11, offset: { x: -50, y: -25, z: 0 } },
    },
  },
  materials: {
    default: {
      name: "Aluminum",
      color: [0.85, 0.85, 0.88],
      metallic: 0.7,
      roughness: 0.4,
    },
  },
  part_materials: {},
  roots: [{ root: 12, material: "default" }],
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Sheet metal U-channel",
    kind: "cube", // there's no "sheet" PartInfo kind yet; cube is the closest neutral choice
    primitiveNodeId: 3,
    scaleNodeId: 10,
    rotateNodeId: 11,
    translateNodeId: 12,
  },
];

export const sheetMetalBracketExample: Example = {
  id: "sheet-metal-bracket",
  name: "Sheet metal U-channel",
  description:
    "100×50×1mm aluminum base flange with two 25mm Up flanges. Demonstrates lossless unfold + per-bend K-factor provenance.",
  difficulty: "intermediate",
  features: ["sheet-metal", "flange", "unfold"],
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 13,
    nextPartNum: 2,
  },
};
