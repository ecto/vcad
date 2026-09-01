import type { Document } from "@vcad/ir";

export interface PlaygroundExample {
  id: string;
  name: string;
  description: string;
  code: string;
  document: Document;
}

/**
 * Pre-built examples for the playground.
 * Each example has both displayable code and the corresponding IR document.
 */
export const examples: PlaygroundExample[] = [
  {
    id: "plate",
    name: "Mounting Plate",
    description: "A simple plate with four mounting holes",
    code: `// plate with four mounting holes
let plate = centered_cube("plate", 100.0, 60.0, 5.0);

let hole = centered_cylinder("hole", 3.0, 10.0, 32);
let holes = hole
  .linear_pattern(80.0, 0.0, 0.0, 2)
  .linear_pattern(0.0, 40.0, 0.0, 2)
  .translate(-40.0, -20.0, 0.0);

let part = plate - holes;
part.write_stl("plate.stl")?;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "plate",
          op: { type: "Cube", size: { x: 100, y: 60, z: 5 } },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: -50, y: -30, z: -2.5 } },
        },
        "3": {
          id: 3,
          name: "hole",
          op: { type: "Cylinder", radius: 3, height: 10, segments: 32 },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: 0, y: 0, z: -5 } },
        },
        "5": {
          id: 5,
          name: null,
          op: {
            type: "LinearPattern",
            child: 4,
            direction: { x: 80, y: 0, z: 0 },
            count: 2,
            spacing: 80,
          },
        },
        "6": {
          id: 6,
          name: null,
          op: {
            type: "LinearPattern",
            child: 5,
            direction: { x: 0, y: 40, z: 0 },
            count: 2,
            spacing: 40,
          },
        },
        "7": {
          id: 7,
          name: null,
          op: { type: "Translate", child: 6, offset: { x: -40, y: -20, z: 0 } },
        },
        "8": {
          id: 8,
          name: "result",
          op: { type: "Difference", left: 2, right: 7 },
        },
      },
      materials: {
        aluminum: {
          name: "Aluminum",
          color: [0.9, 0.9, 0.92],
          metallic: 0.95,
          roughness: 0.3,
          density: 2.7,
        },
      },
      part_materials: {},
      roots: [{ root: 8, material: "aluminum" }],
    },
  },
  {
    id: "bracket",
    name: "L-Bracket",
    description: "An L-shaped mounting bracket",
    code: `// L-bracket with mounting holes
let vertical = centered_cube("v", 40.0, 5.0, 50.0)
  .translate(0.0, 0.0, 25.0);

let horizontal = centered_cube("h", 40.0, 30.0, 5.0)
  .translate(0.0, 12.5, 2.5);

let bracket = vertical + horizontal;

let hole = centered_cylinder("hole", 2.5, 20.0, 24);
let mount_holes = hole
  .rotate(90.0, 0.0, 0.0)
  .linear_pattern(20.0, 0.0, 0.0, 2)
  .translate(-10.0, 0.0, 35.0);

let result = bracket - mount_holes;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "vertical",
          op: { type: "Cube", size: { x: 40, y: 5, z: 50 } },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: -20, y: -2.5, z: 0 } },
        },
        "3": {
          id: 3,
          name: "horizontal",
          op: { type: "Cube", size: { x: 40, y: 30, z: 5 } },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: -20, y: -2.5, z: -5 } },
        },
        "5": {
          id: 5,
          name: null,
          op: { type: "Union", left: 2, right: 4 },
        },
        "6": {
          id: 6,
          name: "hole",
          op: { type: "Cylinder", radius: 2.5, height: 20, segments: 24 },
        },
        "7": {
          id: 7,
          name: null,
          op: { type: "Rotate", child: 6, angles: { x: 90, y: 0, z: 0 } },
        },
        "8": {
          id: 8,
          name: null,
          op: {
            type: "LinearPattern",
            child: 7,
            direction: { x: 20, y: 0, z: 0 },
            count: 2,
            spacing: 20,
          },
        },
        "9": {
          id: 9,
          name: null,
          op: { type: "Translate", child: 8, offset: { x: -10, y: 0, z: 35 } },
        },
        "10": {
          id: 10,
          name: "result",
          op: { type: "Difference", left: 5, right: 9 },
        },
      },
      materials: {
        steel: {
          name: "Steel",
          color: [0.6, 0.6, 0.65],
          metallic: 0.9,
          roughness: 0.4,
          density: 7.8,
        },
      },
      part_materials: {},
      roots: [{ root: 10, material: "steel" }],
    },
  },
  {
    id: "sphere",
    name: "Simple Sphere",
    description: "A basic sphere primitive",
    code: `// simple sphere
let sphere = Part::sphere("ball", 25.0, 32);
sphere.write_stl("sphere.stl")?;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "sphere",
          op: { type: "Sphere", radius: 25, segments: 32 },
        },
      },
      materials: {
        plastic: {
          name: "Plastic",
          color: [0.8, 0.2, 0.1],
          metallic: 0.0,
          roughness: 0.5,
        },
      },
      part_materials: {},
      roots: [{ root: 1, material: "plastic" }],
    },
  },
  {
    id: "cube",
    name: "Hello Cube",
    description: "The simplest vcad example",
    code: `// hello cube
let cube = Part::cube("box", 30.0, 30.0, 30.0);
cube.write_stl("cube.stl")?;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "cube",
          op: { type: "Cube", size: { x: 30, y: 30, z: 30 } },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: -15, y: -15, z: -15 } },
        },
      },
      materials: {
        aluminum: {
          name: "Aluminum",
          color: [0.9, 0.9, 0.92],
          metallic: 0.95,
          roughness: 0.3,
        },
      },
      part_materials: {},
      roots: [{ root: 2, material: "aluminum" }],
    },
  },
  {
    id: "transforms",
    name: "Transforms Demo",
    description: "Translate, rotate, and scale operations",
    code: `// transforms demo
let cube = centered_cube("box", 20.0, 20.0, 20.0);
let rotated = cube.rotate(0.0, 0.0, 45.0);
let moved = rotated.translate(30.0, 0.0, 0.0);`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "cube",
          op: { type: "Cube", size: { x: 20, y: 20, z: 20 } },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: -10, y: -10, z: -10 } },
        },
        "3": {
          id: 3,
          name: null,
          op: { type: "Rotate", child: 2, angles: { x: 0, y: 0, z: 45 } },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: 30, y: 0, z: 0 } },
        },
      },
      materials: {
        plastic: {
          name: "Plastic",
          color: [0.2, 0.6, 0.9],
          metallic: 0.0,
          roughness: 0.5,
        },
      },
      part_materials: {},
      roots: [{ root: 4, material: "plastic" }],
    },
  },
  {
    id: "first-hole",
    name: "First Hole",
    description: "Boolean difference to create a hole",
    code: `// plate with a hole
let plate = centered_cube("plate", 50.0, 50.0, 5.0);
let hole = centered_cylinder("hole", 10.0, 10.0, 32);
let result = plate - hole;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "plate",
          op: { type: "Cube", size: { x: 50, y: 50, z: 5 } },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: -25, y: -25, z: -2.5 } },
        },
        "3": {
          id: 3,
          name: "hole",
          op: { type: "Cylinder", radius: 10, height: 10, segments: 32 },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: 0, y: 0, z: -5 } },
        },
        "5": {
          id: 5,
          name: "result",
          op: { type: "Difference", left: 2, right: 4 },
        },
      },
      materials: {
        aluminum: {
          name: "Aluminum",
          color: [0.9, 0.9, 0.92],
          metallic: 0.95,
          roughness: 0.3,
        },
      },
      part_materials: {},
      roots: [{ root: 5, material: "aluminum" }],
    },
  },
  {
    id: "circular-pattern",
    name: "Circular Pattern",
    description: "Radial vent pattern on a disc",
    code: `// ventilated disc
let disc = centered_cylinder("disc", 35.0, 3.0, 64);
let slot = centered_cube("slot", 15.0, 2.0, 10.0);
let vents = slot.circular_pattern(20.0, 8);
let result = disc - vents;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "disc",
          op: { type: "Cylinder", radius: 35, height: 3, segments: 64 },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: 0, y: 0, z: -1.5 } },
        },
        "3": {
          id: 3,
          name: "slot",
          op: { type: "Cube", size: { x: 15, y: 2, z: 10 } },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: 12.5, y: -1, z: -5 } },
        },
        "5": {
          id: 5,
          name: null,
          op: {
            type: "CircularPattern",
            child: 4,
            axis_origin: { x: 0, y: 0, z: 0 },
            axis_dir: { x: 0, y: 0, z: 1 },
            count: 8,
            angle_deg: 360,
          },
        },
        "6": {
          id: 6,
          name: "result",
          op: { type: "Difference", left: 2, right: 5 },
        },
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
      roots: [{ root: 6, material: "steel" }],
    },
  },
  {
    id: "flanged-hub",
    name: "Flanged Hub",
    description: "Hub with mounting flange and bolt pattern",
    code: `// flanged hub
let hub = centered_cylinder("hub", 15.0, 30.0, 64);
let flange = centered_cylinder("flange", 35.0, 6.0, 64)
  .translate(0.0, 0.0, -15.0);
let bore = centered_cylinder("bore", 6.0, 40.0, 32);
let bolts = bolt_pattern(6, 50.0, 4.0, 10.0, 24)
  .translate(0.0, 0.0, -15.0);
let result = hub + flange - bore - bolts;`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "hub",
          op: { type: "Cylinder", radius: 15, height: 30, segments: 64 },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: 0, y: 0, z: -15 } },
        },
        "3": {
          id: 3,
          name: "flange",
          op: { type: "Cylinder", radius: 35, height: 6, segments: 64 },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: 0, y: 0, z: -18 } },
        },
        "5": {
          id: 5,
          name: null,
          op: { type: "Union", left: 2, right: 4 },
        },
        "6": {
          id: 6,
          name: "bore",
          op: { type: "Cylinder", radius: 6, height: 40, segments: 32 },
        },
        "7": {
          id: 7,
          name: null,
          op: { type: "Translate", child: 6, offset: { x: 0, y: 0, z: -20 } },
        },
        "8": {
          id: 8,
          name: null,
          op: { type: "Difference", left: 5, right: 7 },
        },
        "9": {
          id: 9,
          name: "bolt_hole",
          op: { type: "Cylinder", radius: 4, height: 10, segments: 24 },
        },
        "10": {
          id: 10,
          name: null,
          op: { type: "Translate", child: 9, offset: { x: 25, y: 0, z: -20 } },
        },
        "11": {
          id: 11,
          name: null,
          op: {
            type: "CircularPattern",
            child: 10,
            axis_origin: { x: 0, y: 0, z: 0 },
            axis_dir: { x: 0, y: 0, z: 1 },
            count: 6,
            angle_deg: 360,
          },
        },
        "12": {
          id: 12,
          name: "result",
          op: { type: "Difference", left: 8, right: 11 },
        },
      },
      materials: {
        aluminum: {
          name: "Aluminum",
          color: [0.85, 0.85, 0.88],
          metallic: 0.95,
          roughness: 0.35,
        },
      },
      part_materials: {},
      roots: [{ root: 12, material: "aluminum" }],
    },
  },
  {
    id: "enclosure",
    name: "Electronics Enclosure",
    description: "Box shell with standoffs and vents",
    code: `// electronics enclosure
let wall = 2.0;
let outer = centered_cube("outer", 80.0, 60.0, 40.0);
let inner = centered_cube("inner", 76.0, 56.0, 38.0)
  .translate(0.0, 0.0, 2.0);
let shell = outer - inner;

// Add vent slots
let vent = centered_cube("vent", 2.0, 15.0, 10.0);
let vents = vent.linear_pattern(5.0, 0.0, 0.0, 6);`,
    document: {
      version: "0.1",
      mates: [],
      nodes: {
        "1": {
          id: 1,
          name: "outer",
          op: { type: "Cube", size: { x: 80, y: 60, z: 40 } },
        },
        "2": {
          id: 2,
          name: null,
          op: { type: "Translate", child: 1, offset: { x: -40, y: -30, z: -20 } },
        },
        "3": {
          id: 3,
          name: "inner",
          op: { type: "Cube", size: { x: 76, y: 56, z: 38 } },
        },
        "4": {
          id: 4,
          name: null,
          op: { type: "Translate", child: 3, offset: { x: -38, y: -28, z: -18 } },
        },
        "5": {
          id: 5,
          name: "shell",
          op: { type: "Difference", left: 2, right: 4 },
        },
        "6": {
          id: 6,
          name: "vent",
          op: { type: "Cube", size: { x: 2, y: 60, z: 15 } },
        },
        "7": {
          id: 7,
          name: null,
          op: { type: "Translate", child: 6, offset: { x: -12.5, y: -30, z: 5 } },
        },
        "8": {
          id: 8,
          name: null,
          op: {
            type: "LinearPattern",
            child: 7,
            direction: { x: 25, y: 0, z: 0 },
            count: 6,
            spacing: 5,
          },
        },
        "9": {
          id: 9,
          name: "result",
          op: { type: "Difference", left: 5, right: 8 },
        },
      },
      materials: {
        abs: {
          name: "ABS Plastic",
          color: [0.15, 0.15, 0.15],
          metallic: 0.0,
          roughness: 0.6,
        },
      },
      part_materials: {},
      roots: [{ root: 9, material: "abs" }],
    },
  },
];

export function getExample(id: string): PlaygroundExample | undefined {
  return examples.find(e => e.id === id);
}
