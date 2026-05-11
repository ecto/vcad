import type { Example } from "./index";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Sheet-metal U-channel — base flange (100×50, 1mm aluminum) plus two
// 25mm Up edge flanges along the long edges. Evaluated by routing this
// IR chain through `kernelWasm.evaluateSheetMetalChain`; both the bent 3D
// mesh and the flat pattern come from the Rust kernel.
const document: Document = {
  version: "0.1",
  nodes: {
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
    kind: "cube", // no PartInfo kind for sheet metal yet
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
    "100×50×1mm aluminum base flange with two 25mm Up flanges. Evaluated entirely in the Rust kernel via @vcad/kernel-wasm.",
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
