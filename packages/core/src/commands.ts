import type { PrimitiveKind, BooleanType, TransformMode } from "./types.js";
import type { JointKind } from "@vcad/ir";

export interface Command {
  id: string;
  label: string;
  icon: string;
  keywords: string[];
  shortcut?: string;
  action: () => void;
  enabled?: () => boolean;
}

export type CommandRegistry = Command[];

/** Core actions required by all consumers. */
export interface CommandActions {
  addPrimitive: (kind: PrimitiveKind) => void;
  applyBoolean: (type: BooleanType) => void;
  setTransformMode: (mode: TransformMode) => void;
  undo: () => void;
  redo: () => void;
  toggleWireframe: () => void;
  toggleGridSnap: () => void;
  toggleFeatureTree: () => void;
  save: () => void;
  open: () => void;
  exportStl: () => void;
  exportGlb: () => void;
  openAbout: () => void;
  deleteSelected: () => void;
  duplicateSelected: () => void;
  deselectAll: () => void;
  hasTwoSelected: () => boolean;
  hasSelection: () => boolean;
  hasParts: () => boolean;
  canUndo: () => boolean;
  canRedo: () => boolean;
  // Assembly actions (optional — only needed by full app)
  createPartDef?: () => void;
  insertInstance?: () => void;
  addJoint?: (kind: JointKind) => void;
  setGroundInstance?: () => void;
  hasOnePartSelected?: () => boolean;
  hasPartDefs?: () => boolean;
  hasTwoInstancesSelected?: () => boolean;
  hasOneInstanceSelected?: () => boolean;
  // Modify operations (optional — only needed by full app)
  applyFillet?: () => void;
  applyChamfer?: () => void;
  applyShell?: () => void;
  applyLinearPattern?: () => void;
  applyCircularPattern?: () => void;
  applyMirror?: () => void;
  applyStitch?: () => void;
  // Electronics (optional)
  enterElectronics?: () => void;
  exitElectronics?: () => void;
  hasPcb?: () => boolean;
}

const noop = () => {};
const alwaysFalse = () => false;

export function createCommandRegistry(actions: CommandActions): CommandRegistry {
  const cmds: CommandRegistry = [
    // Primitives
    {
      id: "add-box",
      label: "Add Box",
      icon: "Cube",
      keywords: ["box", "cube", "primitive", "create", "add"],
      action: () => actions.addPrimitive("cube"),
    },
    {
      id: "add-cylinder",
      label: "Add Cylinder",
      icon: "Cylinder",
      keywords: ["cylinder", "primitive", "create", "add", "tube"],
      action: () => actions.addPrimitive("cylinder"),
    },
    {
      id: "add-sphere",
      label: "Add Sphere",
      icon: "Globe",
      keywords: ["sphere", "ball", "primitive", "create", "add"],
      action: () => actions.addPrimitive("sphere"),
    },

    // Booleans
    {
      id: "boolean-union",
      label: "Union",
      icon: "Unite",
      keywords: ["union", "combine", "add", "boolean", "merge"],
      shortcut: "Ctrl+Shift+U",
      action: () => actions.applyBoolean("union"),
      enabled: actions.hasTwoSelected,
    },
    {
      id: "boolean-difference",
      label: "Difference",
      icon: "Subtract",
      keywords: ["difference", "subtract", "cut", "boolean", "minus"],
      shortcut: "Ctrl+Shift+D",
      action: () => actions.applyBoolean("difference"),
      enabled: actions.hasTwoSelected,
    },
    {
      id: "boolean-intersection",
      label: "Intersection",
      icon: "Intersect",
      keywords: ["intersection", "intersect", "boolean", "and"],
      shortcut: "Ctrl+Shift+I",
      action: () => actions.applyBoolean("intersection"),
      enabled: actions.hasTwoSelected,
    },

    // Transform modes
    {
      id: "mode-move",
      label: "Move Mode",
      icon: "ArrowsOutCardinal",
      keywords: ["move", "translate", "position", "transform"],
      shortcut: "M",
      action: () => actions.setTransformMode("translate"),
    },
    {
      id: "mode-rotate",
      label: "Rotate Mode",
      icon: "ArrowClockwise",
      keywords: ["rotate", "spin", "turn", "transform"],
      shortcut: "R",
      action: () => actions.setTransformMode("rotate"),
    },
    {
      id: "mode-scale",
      label: "Scale Mode",
      icon: "ArrowsOut",
      keywords: ["scale", "resize", "size", "transform"],
      shortcut: "Shift+S",
      action: () => actions.setTransformMode("scale"),
    },

    // Edit operations
    {
      id: "undo",
      label: "Undo",
      icon: "ArrowCounterClockwise",
      keywords: ["undo", "back", "revert"],
      shortcut: "Ctrl+Z",
      action: actions.undo,
      enabled: actions.canUndo,
    },
    {
      id: "redo",
      label: "Redo",
      icon: "ArrowClockwise",
      keywords: ["redo", "forward"],
      shortcut: "Ctrl+Shift+Z",
      action: actions.redo,
      enabled: actions.canRedo,
    },
    {
      id: "delete",
      label: "Delete Selected",
      icon: "Trash",
      keywords: ["delete", "remove", "trash"],
      shortcut: "Backspace",
      action: actions.deleteSelected,
      enabled: actions.hasSelection,
    },
    {
      id: "duplicate",
      label: "Duplicate",
      icon: "Copy",
      keywords: ["duplicate", "copy", "clone"],
      shortcut: "Ctrl+D",
      action: actions.duplicateSelected,
      enabled: actions.hasSelection,
    },
    {
      id: "deselect",
      label: "Deselect All",
      icon: "X",
      keywords: ["deselect", "clear", "none"],
      shortcut: "Esc",
      action: actions.deselectAll,
    },

    // View toggles
    {
      id: "toggle-wireframe",
      label: "Toggle Wireframe",
      icon: "CubeTransparent",
      keywords: ["wireframe", "edges", "view"],
      shortcut: "X",
      action: actions.toggleWireframe,
    },
    {
      id: "toggle-grid-snap",
      label: "Toggle Grid Snap",
      icon: "GridFour",
      keywords: ["snap", "grid", "align"],
      shortcut: "G",
      action: actions.toggleGridSnap,
    },
    {
      id: "toggle-sidebar",
      label: "Toggle Sidebar",
      icon: "SidebarSimple",
      keywords: ["sidebar", "panel", "tree", "features"],
      action: actions.toggleFeatureTree,
    },

    // File operations
    {
      id: "save",
      label: "Save",
      icon: "FloppyDisk",
      keywords: ["save", "export", "file"],
      shortcut: "Ctrl+S",
      action: actions.save,
    },
    {
      id: "open",
      label: "Open",
      icon: "FolderOpen",
      keywords: ["open", "load", "file", "import"],
      shortcut: "Ctrl+O",
      action: actions.open,
    },
    {
      id: "export-stl",
      label: "Export STL",
      icon: "Export",
      keywords: ["export", "stl", "mesh", "3d print"],
      action: actions.exportStl,
      enabled: actions.hasParts,
    },
    {
      id: "export-glb",
      label: "Export GLB",
      icon: "Export",
      keywords: ["export", "glb", "gltf", "mesh"],
      action: actions.exportGlb,
      enabled: actions.hasParts,
    },

    // Help
    {
      id: "about",
      label: "About vcad",
      icon: "Info",
      keywords: ["about", "help", "info", "version"],
      action: actions.openAbout,
    },
  ];

  // Assembly commands — only added when actions are provided
  if (actions.createPartDef) {
    cmds.push({
      id: "create-part-def",
      label: "Create Part Definition",
      icon: "Package",
      keywords: ["part", "definition", "assembly", "create", "convert"],
      action: actions.createPartDef,
      enabled: actions.hasOnePartSelected ?? alwaysFalse,
    });
  }
  if (actions.insertInstance) {
    cmds.push({
      id: "insert-instance",
      label: "Insert Instance",
      icon: "PlusSquare",
      keywords: ["insert", "instance", "assembly", "add", "part"],
      action: actions.insertInstance,
      enabled: actions.hasPartDefs ?? alwaysFalse,
    });
  }
  if (actions.addJoint) {
    const addJoint = actions.addJoint;
    const enabledTwoInst = actions.hasTwoInstancesSelected ?? alwaysFalse;
    cmds.push(
      {
        id: "add-fixed-joint",
        label: "Add Fixed Joint",
        icon: "Anchor",
        keywords: ["joint", "fixed", "assembly", "connect", "weld"],
        action: () => addJoint({ type: "Fixed" }),
        enabled: enabledTwoInst,
      },
      {
        id: "add-revolute-joint",
        label: "Add Revolute Joint",
        icon: "ArrowsClockwise",
        keywords: ["joint", "revolute", "hinge", "assembly", "rotate"],
        action: () => addJoint({ type: "Revolute", axis: { x: 0, y: 0, z: 1 } }),
        enabled: enabledTwoInst,
      },
      {
        id: "add-slider-joint",
        label: "Add Slider Joint",
        icon: "ArrowsHorizontal",
        keywords: ["joint", "slider", "prismatic", "assembly", "slide"],
        action: () => addJoint({ type: "Slider", axis: { x: 0, y: 0, z: 1 } }),
        enabled: enabledTwoInst,
      },
    );
  }
  if (actions.setGroundInstance) {
    cmds.push({
      id: "set-ground",
      label: "Set as Ground",
      icon: "Anchor",
      keywords: ["ground", "fix", "base", "assembly", "anchor"],
      action: actions.setGroundInstance,
      enabled: actions.hasOneInstanceSelected ?? alwaysFalse,
    });
  }

  // Modify operations — only added when actions are provided
  const enabledOnePart = actions.hasOnePartSelected ?? alwaysFalse;
  if (actions.applyFillet) {
    cmds.push({
      id: "apply-fillet",
      label: "Fillet",
      icon: "Circle",
      keywords: ["fillet", "round", "radius", "edge"],
      action: actions.applyFillet,
      enabled: enabledOnePart,
    });
  }
  if (actions.applyChamfer) {
    cmds.push({
      id: "apply-chamfer",
      label: "Chamfer",
      icon: "Octagon",
      keywords: ["chamfer", "bevel", "edge", "corner"],
      action: actions.applyChamfer,
      enabled: enabledOnePart,
    });
  }
  if (actions.applyShell) {
    cmds.push({
      id: "apply-shell",
      label: "Shell",
      icon: "Cube",
      keywords: ["shell", "hollow", "thickness", "wall"],
      action: actions.applyShell,
      enabled: enabledOnePart,
    });
  }
  if (actions.applyLinearPattern) {
    cmds.push({
      id: "apply-linear-pattern",
      label: "Linear Pattern",
      icon: "DotsThree",
      keywords: ["pattern", "linear", "array", "repeat", "copy"],
      action: actions.applyLinearPattern,
      enabled: enabledOnePart,
    });
  }
  if (actions.applyCircularPattern) {
    cmds.push({
      id: "apply-circular-pattern",
      label: "Circular Pattern",
      icon: "CircleNotch",
      keywords: ["pattern", "circular", "radial", "array", "repeat"],
      action: actions.applyCircularPattern,
      enabled: enabledOnePart,
    });
  }
  if (actions.applyMirror) {
    cmds.push({
      id: "apply-mirror",
      label: "Mirror",
      icon: "ArrowsHorizontal",
      keywords: ["mirror", "reflect", "flip", "symmetry"],
      action: actions.applyMirror,
      enabled: enabledOnePart,
    });
  }
  if (actions.applyStitch) {
    cmds.push({
      id: "apply-stitch",
      label: "Stitch",
      icon: "Scissors",
      keywords: ["stitch", "embroidery", "sew", "embroider"],
      action: actions.applyStitch,
      enabled: enabledOnePart,
    });
  }

  // Electronics commands
  if (actions.enterElectronics) {
    cmds.push({
      id: "enter-electronics",
      label: "Open Electronics",
      icon: "CircuitBoard",
      keywords: ["electronics", "pcb", "schematic", "board", "ecad"],
      action: actions.enterElectronics,
      enabled: actions.hasPcb ?? alwaysFalse,
    });
  }
  if (actions.exitElectronics) {
    cmds.push({
      id: "exit-electronics",
      label: "Close Electronics",
      icon: "X",
      keywords: ["electronics", "close", "exit", "pcb"],
      action: actions.exitElectronics,
    });
  }

  return cmds;
}
