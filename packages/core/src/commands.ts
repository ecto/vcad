import type { PrimitiveKind, BooleanType, TransformMode } from "./types.js";
import type { JointKind } from "@vcad/ir";

/**
 * Grouping buckets used by surfaces that render the command registry as a
 * menu (mobile hamburger sheet, desktop header — future). Matches the classic
 * Borland-style menu bar layout so desktop and mobile can stay aligned.
 */
export type CommandCategory =
  | "file"
  | "edit"
  | "view"
  | "create"
  | "modify"
  | "assembly"
  | "tools"
  | "help";

export interface Command {
  id: string;
  label: string;
  icon: string;
  keywords: string[];
  shortcut?: string;
  action: () => void;
  enabled?: () => boolean;
  /** Menu category used by MobileShell (and future Header refactor) to group
   * commands into sections. Undefined = don't show in menus, only palette. */
  category?: CommandCategory;
}

export type CommandRegistry = Command[];

/** Ordered list of categories for menu rendering. Any category not in this
 * list won't appear in the menu (the command palette shows everything). */
export const COMMAND_CATEGORIES: CommandCategory[] = [
  "file",
  "edit",
  "create",
  "modify",
  "assembly",
  "view",
  "tools",
  "help",
];

export const CATEGORY_LABELS: Record<CommandCategory, string> = {
  file: "File",
  edit: "Edit",
  create: "Create",
  modify: "Modify",
  assembly: "Assembly",
  view: "View",
  tools: "Tools",
  help: "Help",
};

/** Tailwind color classes used by menu surfaces to tint command icons by
 * category. Keeps a monokai-adjacent palette so the chrome feels cohesive
 * with vcad's existing brand pink without leaning on it for every row. */
export const CATEGORY_ICON_COLORS: Record<CommandCategory, string> = {
  file: "text-sky-400",
  edit: "text-orange-400",
  create: "text-cyan-400",
  modify: "text-green-400",
  assembly: "text-purple-400",
  view: "text-yellow-400",
  tools: "text-brand",
  help: "text-text-muted",
};

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
  // File — optional extras
  openFromCloud?: () => void;
  exportStep?: () => void;
  newDocument?: () => void;
  // Edit — optional extras
  copy?: () => void;
  paste?: () => void;
  selectAll?: () => void;
  // View — optional extras (all app-layer)
  cameraFit?: () => void;
  cameraPreset?: (preset: "isometric" | "top" | "front" | "right") => void;
  toggleChatSidebar?: () => void;
  toggleStatusBar?: () => void;
  toggleDevTools?: () => void;
  cycleTheme?: () => void;
  // Tools — optional extras
  openCommandPalette?: () => void;
  newSketch?: () => void;
  openSlicer?: () => void;
  openCam?: () => void;
  // Help — optional extras
  openDocs?: () => void;
  openGithub?: () => void;
  openDiscord?: () => void;
  openWhatsNew?: () => void;
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
      category: "create",
    },
    {
      id: "add-cylinder",
      label: "Add Cylinder",
      icon: "Cylinder",
      keywords: ["cylinder", "primitive", "create", "add", "tube"],
      action: () => actions.addPrimitive("cylinder"),
      category: "create",
    },
    {
      id: "add-sphere",
      label: "Add Sphere",
      icon: "Globe",
      keywords: ["sphere", "ball", "primitive", "create", "add"],
      action: () => actions.addPrimitive("sphere"),
      category: "create",
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
      category: "modify",
    },
    {
      id: "boolean-difference",
      label: "Difference",
      icon: "Subtract",
      keywords: ["difference", "subtract", "cut", "boolean", "minus"],
      shortcut: "Ctrl+Shift+D",
      action: () => actions.applyBoolean("difference"),
      enabled: actions.hasTwoSelected,
      category: "modify",
    },
    {
      id: "boolean-intersection",
      label: "Intersection",
      icon: "Intersect",
      keywords: ["intersection", "intersect", "boolean", "and"],
      shortcut: "Ctrl+Shift+I",
      action: () => actions.applyBoolean("intersection"),
      enabled: actions.hasTwoSelected,
      category: "modify",
    },

    // Transform modes
    {
      id: "mode-move",
      label: "Move Mode",
      icon: "ArrowsOutCardinal",
      keywords: ["move", "translate", "position", "transform"],
      shortcut: "M",
      action: () => actions.setTransformMode("translate"),
      category: "edit",
    },
    {
      id: "mode-rotate",
      label: "Rotate Mode",
      icon: "ArrowClockwise",
      keywords: ["rotate", "spin", "turn", "transform"],
      shortcut: "R",
      action: () => actions.setTransformMode("rotate"),
      category: "edit",
    },
    {
      id: "mode-scale",
      label: "Scale Mode",
      icon: "ArrowsOut",
      keywords: ["scale", "resize", "size", "transform"],
      shortcut: "Shift+S",
      action: () => actions.setTransformMode("scale"),
      category: "edit",
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
      category: "edit",
    },
    {
      id: "redo",
      label: "Redo",
      icon: "ArrowClockwise",
      keywords: ["redo", "forward"],
      shortcut: "Ctrl+Shift+Z",
      action: actions.redo,
      enabled: actions.canRedo,
      category: "edit",
    },
    {
      id: "delete",
      label: "Delete Selected",
      icon: "Trash",
      keywords: ["delete", "remove", "trash"],
      shortcut: "Backspace",
      action: actions.deleteSelected,
      enabled: actions.hasSelection,
      category: "edit",
    },
    {
      id: "duplicate",
      label: "Duplicate",
      icon: "Copy",
      keywords: ["duplicate", "copy", "clone"],
      shortcut: "Ctrl+D",
      action: actions.duplicateSelected,
      enabled: actions.hasSelection,
      category: "edit",
    },
    {
      id: "deselect",
      label: "Deselect All",
      icon: "X",
      keywords: ["deselect", "clear", "none"],
      shortcut: "Esc",
      action: actions.deselectAll,
      category: "edit",
    },

    // View toggles
    {
      id: "toggle-wireframe",
      label: "Toggle Wireframe",
      icon: "CubeTransparent",
      keywords: ["wireframe", "edges", "view"],
      shortcut: "X",
      action: actions.toggleWireframe,
      category: "view",
    },
    {
      id: "toggle-grid-snap",
      label: "Toggle Grid Snap",
      icon: "GridFour",
      keywords: ["snap", "grid", "align"],
      shortcut: "G",
      action: actions.toggleGridSnap,
      category: "view",
    },
    {
      id: "toggle-sidebar",
      label: "Toggle Feature Tree",
      icon: "SidebarSimple",
      keywords: ["sidebar", "panel", "tree", "features"],
      action: actions.toggleFeatureTree,
      category: "view",
    },

    // File operations
    {
      id: "save",
      label: "Save",
      icon: "FloppyDisk",
      keywords: ["save", "export", "file"],
      shortcut: "Ctrl+S",
      action: actions.save,
      category: "file",
    },
    {
      id: "open",
      label: "Open",
      icon: "FolderOpen",
      keywords: ["open", "load", "file", "import"],
      shortcut: "Ctrl+O",
      action: actions.open,
      category: "file",
    },
    {
      id: "export-stl",
      label: "Export STL",
      icon: "Export",
      keywords: ["export", "stl", "mesh", "3d print"],
      action: actions.exportStl,
      enabled: actions.hasParts,
      category: "file",
    },
    {
      id: "export-glb",
      label: "Export GLB",
      icon: "Export",
      keywords: ["export", "glb", "gltf", "mesh"],
      action: actions.exportGlb,
      enabled: actions.hasParts,
      category: "file",
    },

    // Help
    {
      id: "about",
      label: "About vcad",
      icon: "Info",
      keywords: ["about", "help", "info", "version"],
      action: actions.openAbout,
      category: "help",
    },
  ];

  // ---------- File extras ----------
  if (actions.newDocument) {
    cmds.push({
      id: "new-document",
      label: "New",
      icon: "FilePlus",
      keywords: ["new", "document", "blank", "fresh"],
      shortcut: "Ctrl+N",
      action: actions.newDocument,
      category: "file",
    });
  }
  if (actions.openFromCloud) {
    cmds.push({
      id: "open-cloud",
      label: "Open from Cloud…",
      icon: "CloudArrowDown",
      keywords: ["open", "cloud", "load", "sync", "remote"],
      shortcut: "Ctrl+Shift+O",
      action: actions.openFromCloud,
      category: "file",
    });
  }
  if (actions.exportStep) {
    cmds.push({
      id: "export-step",
      label: "Export STEP",
      icon: "Export",
      keywords: ["export", "step", "cad", "solidworks", "fusion"],
      action: actions.exportStep,
      enabled: actions.hasParts,
      category: "file",
    });
  }

  // ---------- Edit extras ----------
  if (actions.copy) {
    cmds.push({
      id: "copy",
      label: "Copy",
      icon: "Copy",
      keywords: ["copy", "clipboard"],
      shortcut: "Ctrl+C",
      action: actions.copy,
      enabled: actions.hasSelection,
      category: "edit",
    });
  }
  if (actions.paste) {
    cmds.push({
      id: "paste",
      label: "Paste",
      icon: "ClipboardText",
      keywords: ["paste", "clipboard"],
      shortcut: "Ctrl+V",
      action: actions.paste,
      category: "edit",
    });
  }
  if (actions.selectAll) {
    cmds.push({
      id: "select-all",
      label: "Select All",
      icon: "Selection",
      keywords: ["select", "all", "everything"],
      shortcut: "Ctrl+A",
      action: actions.selectAll,
      category: "edit",
    });
  }

  // ---------- View extras ----------
  if (actions.cameraFit) {
    cmds.push({
      id: "camera-fit",
      label: "Fit to View",
      icon: "ArrowsOutCardinal",
      keywords: ["fit", "zoom", "frame", "view", "camera"],
      shortcut: "F",
      action: actions.cameraFit,
      category: "view",
    });
  }
  if (actions.cameraPreset) {
    const preset = actions.cameraPreset;
    cmds.push(
      {
        id: "camera-isometric",
        label: "Isometric View",
        icon: "Cube",
        keywords: ["isometric", "iso", "view", "camera", "angle"],
        action: () => preset("isometric"),
        category: "view",
      },
      {
        id: "camera-top",
        label: "Top View",
        icon: "ArrowUp",
        keywords: ["top", "view", "camera", "angle", "plan"],
        action: () => preset("top"),
        category: "view",
      },
      {
        id: "camera-front",
        label: "Front View",
        icon: "ArrowRight",
        keywords: ["front", "view", "camera", "angle"],
        action: () => preset("front"),
        category: "view",
      },
      {
        id: "camera-right",
        label: "Right View",
        icon: "ArrowRight",
        keywords: ["right", "side", "view", "camera", "angle"],
        action: () => preset("right"),
        category: "view",
      },
    );
  }
  if (actions.toggleChatSidebar) {
    cmds.push({
      id: "toggle-chat",
      label: "Toggle Chat",
      icon: "ChatDots",
      keywords: ["chat", "sidebar", "ai", "assistant"],
      shortcut: "F6",
      action: actions.toggleChatSidebar,
      category: "view",
    });
  }
  if (actions.toggleStatusBar) {
    cmds.push({
      id: "toggle-status-bar",
      label: "Toggle Status Bar",
      icon: "Terminal",
      keywords: ["status", "bar", "toggle", "view"],
      action: actions.toggleStatusBar,
      category: "view",
    });
  }
  if (actions.toggleDevTools) {
    cmds.push({
      id: "toggle-devtools",
      label: "Toggle DevTools",
      icon: "Terminal",
      keywords: ["devtools", "console", "log", "debug"],
      shortcut: "`",
      action: actions.toggleDevTools,
      category: "view",
    });
  }
  if (actions.cycleTheme) {
    cmds.push({
      id: "cycle-theme",
      label: "Cycle Theme",
      icon: "Sun",
      keywords: ["theme", "dark", "light", "system", "mode"],
      action: actions.cycleTheme,
      category: "view",
    });
  }

  // ---------- Tools extras ----------
  if (actions.openCommandPalette) {
    cmds.push({
      id: "command-palette",
      label: "Command Palette…",
      icon: "Command",
      keywords: ["command", "palette", "search", "go"],
      shortcut: "Ctrl+K",
      action: actions.openCommandPalette,
      category: "tools",
    });
  }
  if (actions.newSketch) {
    cmds.push({
      id: "new-sketch",
      label: "New Sketch…",
      icon: "Pencil",
      keywords: ["sketch", "2d", "draw", "profile"],
      action: actions.newSketch,
      category: "tools",
    });
  }
  if (actions.openSlicer) {
    cmds.push({
      id: "open-slicer",
      label: "Print (Slicer)…",
      icon: "Printer",
      keywords: ["print", "slicer", "3d", "gcode"],
      action: actions.openSlicer,
      enabled: actions.hasParts,
      category: "tools",
    });
  }
  if (actions.openCam) {
    cmds.push({
      id: "open-cam",
      label: "CAM (Toolpath)…",
      icon: "Wrench",
      keywords: ["cam", "toolpath", "mill", "cnc"],
      action: actions.openCam,
      enabled: actions.hasParts,
      category: "tools",
    });
  }

  // ---------- Help extras ----------
  if (actions.openWhatsNew) {
    cmds.push({
      id: "whats-new",
      label: "What's New",
      icon: "Rocket",
      keywords: ["changelog", "whats", "new", "release", "updates"],
      action: actions.openWhatsNew,
      category: "help",
    });
  }
  if (actions.openDocs) {
    cmds.push({
      id: "open-docs",
      label: "Documentation",
      icon: "BookOpen",
      keywords: ["docs", "documentation", "help", "manual", "guide"],
      action: actions.openDocs,
      category: "help",
    });
  }
  if (actions.openGithub) {
    cmds.push({
      id: "open-github",
      label: "GitHub",
      icon: "GithubLogo",
      keywords: ["github", "source", "code", "repo"],
      action: actions.openGithub,
      category: "help",
    });
  }
  if (actions.openDiscord) {
    cmds.push({
      id: "open-discord",
      label: "Discord",
      icon: "DiscordLogo",
      keywords: ["discord", "chat", "community"],
      action: actions.openDiscord,
      category: "help",
    });
  }

  // Assembly commands — only added when actions are provided
  if (actions.createPartDef) {
    cmds.push({
      id: "create-part-def",
      label: "Create Part Definition",
      icon: "Package",
      keywords: ["part", "definition", "assembly", "create", "convert"],
      action: actions.createPartDef,
      enabled: actions.hasOnePartSelected ?? alwaysFalse,
      category: "assembly",
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
      category: "assembly",
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
        category: "assembly",
      },
      {
        id: "add-revolute-joint",
        label: "Add Revolute Joint",
        icon: "ArrowsClockwise",
        keywords: ["joint", "revolute", "hinge", "assembly", "rotate"],
        action: () => addJoint({ type: "Revolute", axis: { x: 0, y: 0, z: 1 } }),
        enabled: enabledTwoInst,
        category: "assembly",
      },
      {
        id: "add-slider-joint",
        label: "Add Slider Joint",
        icon: "ArrowsHorizontal",
        keywords: ["joint", "slider", "prismatic", "assembly", "slide"],
        action: () => addJoint({ type: "Slider", axis: { x: 0, y: 0, z: 1 } }),
        enabled: enabledTwoInst,
        category: "assembly",
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
      category: "assembly",
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
      category: "modify",
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
      category: "modify",
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
      category: "modify",
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
      category: "modify",
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
      category: "modify",
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
      category: "modify",
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
      category: "modify",
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
      category: "tools",
    });
  }
  if (actions.exitElectronics) {
    cmds.push({
      id: "exit-electronics",
      label: "Close Electronics",
      icon: "X",
      keywords: ["electronics", "close", "exit", "pcb"],
      action: actions.exitElectronics,
      category: "tools",
    });
  }

  return cmds;
}
