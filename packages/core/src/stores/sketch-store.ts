import { create } from "zustand";
import type { Vec2, Vec3, SketchSegment2D, SketchConstraint } from "@vcad/ir";
import type { SketchPlane, SketchState, ConstraintTool, ConstraintStatus, FaceInfo } from "../types.js";
import { computePlaneFromFace, getSketchPlaneDirections } from "../types.js";
import { getKernelWasm, getKernelWasmSync } from "../wasm-singleton.js";
import { buildRectangle, buildCircle } from "../sketch-math.js";

/** A saved profile snapshot for loft operations */
export interface ProfileSnapshot {
  id: string;
  plane: SketchPlane;
  origin: Vec3;
  segments: SketchSegment2D[];
}

export type SketchExitStatus = "cancelled" | "empty" | "has_segments";

/**
 * The downstream operation a user is building toward — set by `beginOperation`
 * before or during a sketch. The PropertyManager renders these params live and
 * the live-preview hook reads them on every sketch edit. `commitOperation`
 * runs the actual `addExtrude` / `addRevolve` / etc. call on the document.
 */
export type PendingOperation =
  | { kind: "extrude"; depth: number; flip: boolean; twistDeg: number; scaleEnd: number }
  | { kind: "revolve"; angleDeg: number; flip: boolean }
  | {
      kind: "sweep";
      pathType: "line" | "helix";
      height: number;
      radius: number;
      turns: number;
    }
  | { kind: "loft"; closed: boolean };

/** Undo snapshot for sketch-local history — captured before each mutation. */
interface SketchSnapshot {
  segments: SketchSegment2D[];
  constraints: SketchConstraint[];
  points: Vec2[];
  selectedSegments: number[];
  solved: boolean;
  constraintStatus: ConstraintStatus;
}

const MAX_SKETCH_HISTORY = 100;

export interface SketchStore extends SketchState {
  // Confirmation state
  pendingExit: boolean;

  // Sketch-local undo/redo stacks (cleared on enter/exit sketch)
  history: SketchSnapshot[];
  future: SketchSnapshot[];

  // Face selection state
  faceSelectionMode: boolean;
  hoveredFace: FaceInfo | null;
  selectedFace: FaceInfo | null;

  // 3D cursor state for integrated sketch rendering
  cursorWorldPos: Vec3 | null;
  cursorSketchPos: Vec2 | null;
  snapTarget: Vec2 | null;

  // Selection of a single sketch entity (segment or constraint) for the
  // PropertyManager. `null` when the panel should show tool-default content.
  selectedConstraintIndex: number | null;

  // The downstream operation the user is building toward, if any. Drives the
  // operation params section of the PropertyManager *and* the continuous
  // downstream preview hook.
  pendingOperation: PendingOperation | null;

  // Actions
  undoSketch: () => void;
  redoSketch: () => void;
  validateState: () => boolean; // Returns true if state was fixed
  enterFaceSelectionMode: () => void;
  setHoveredFace: (face: FaceInfo | null) => void;
  selectFace: (face: FaceInfo) => void;
  cancelFaceSelection: () => void;
  enterSketchMode: (plane: SketchPlane, origin?: Vec3, flipped?: boolean) => void;
  exitSketchMode: () => SketchExitStatus;
  requestExit: () => boolean; // Returns true if immediate exit, false if needs confirmation
  confirmExit: () => SketchExitStatus;
  cancelExit: () => void;
  setTool: (tool: SketchState["tool"]) => void;
  addPoint: (point: Vec2) => void;
  finishShape: () => void;
  clearSketch: () => void;
  addRectangle: (p1: Vec2, p2: Vec2) => void;
  addCircle: (center: Vec2, radius: number, segments?: number) => void;
  // Constraint actions
  setConstraintTool: (tool: ConstraintTool) => void;
  toggleSegmentSelection: (index: number) => void;
  clearSelection: () => void;
  addConstraint: (constraint: SketchConstraint) => void;
  removeConstraint: (index: number) => void;
  solveSketch: () => void;
  // Apply specific constraints
  applyHorizontal: () => void;
  applyVertical: () => void;
  applyDistance: (distance: number) => void;
  applyLength: (length: number) => void;
  applyParallel: () => void;
  applyPerpendicular: () => void;
  applyEqual: () => void;
  // Loft mode actions
  loftMode: boolean;
  profiles: ProfileSnapshot[];
  enterLoftMode: (plane: SketchPlane) => void;
  saveProfile: () => void;
  clearForNextProfile: (newOrigin: Vec3) => void;
  exitLoftMode: () => ProfileSnapshot[] | null;
  // 3D cursor actions
  setCursorPos: (world: Vec3 | null, sketch: Vec2 | null, snap: Vec2 | null) => void;
  // PropertyManager focus
  setSelectedConstraint: (index: number | null) => void;
  // Pending downstream operation (Extrude / Revolve / Sweep / Loft)
  beginOperation: (op: PendingOperation) => void;
  updateOperation: (patch: Partial<PendingOperation>) => void;
  clearOperation: () => void;
}

const DEFAULT_PENDING_OPERATION: Record<PendingOperation["kind"], PendingOperation> = {
  extrude: { kind: "extrude", depth: 20, flip: false, twistDeg: 0, scaleEnd: 1.0 },
  revolve: { kind: "revolve", angleDeg: 360, flip: false },
  sweep: { kind: "sweep", pathType: "line", height: 20, radius: 10, turns: 2 },
  loft: { kind: "loft", closed: false },
};

// Shape builders are delegated to `@vcad/core/sketch-math` so the web
// app, the TUI, and the WASM SketchSession all generate identical
// geometry. These locals just thread the store's call sites.
const makeRectangleSegments = (p1: Vec2, p2: Vec2): SketchSegment2D[] =>
  buildRectangle(p1, p2);

const makeCircleSegments = (
  center: Vec2,
  radius: number,
  n: number = 32,
): SketchSegment2D[] => buildCircle(center, radius, n);

let profileIdCounter = 0;

/** Compute constraint status based on current state */
function computeConstraintStatus(
  hasSegments: boolean,
  constraints: SketchConstraint[],
  solved: boolean
): ConstraintStatus {
  // No segments means nothing to constrain
  if (!hasSegments) return "under";

  // No constraints = under-constrained
  if (constraints.length === 0) return "under";

  // Constraints exist but not solved = error (conflicting)
  if (!solved) return "error";

  // Constraints exist and solved = fully constrained
  return "solved";
}

/** Capture the parts of state that sketch-local undo should restore. */
function snapshot(s: SketchStore): SketchSnapshot {
  return {
    segments: [...s.segments],
    constraints: [...s.constraints],
    points: [...s.points],
    selectedSegments: [...s.selectedSegments],
    solved: s.solved,
    constraintStatus: s.constraintStatus,
  };
}

export const useSketchStore = create<SketchStore>((set, get) => {
  /** Push current state onto the undo stack and clear redo. */
  function pushHistory() {
    const state = get();
    const next = [...state.history, snapshot(state)];
    if (next.length > MAX_SKETCH_HISTORY) next.shift();
    set({ history: next, future: [] });
  }

  return {
  active: false,
  plane: "XY",
  origin: { x: 0, y: 0, z: 0 },
  segments: [],
  constraints: [],
  tool: "rectangle",
  constraintTool: "none",
  points: [],
  selectedSegments: [],
  solved: true,
  constraintStatus: "under",
  loftMode: false,
  profiles: [],
  pendingExit: false,
  history: [],
  future: [],
  faceSelectionMode: false,
  hoveredFace: null,
  selectedFace: null,
  cursorWorldPos: null,
  cursorSketchPos: null,
  snapTarget: null,
  selectedConstraintIndex: null,
  pendingOperation: null,

  undoSketch: () => {
    const state = get();
    if (state.history.length === 0) return;
    const prev = state.history[state.history.length - 1]!;
    const newHistory = state.history.slice(0, -1);
    const newFuture = [...state.future, snapshot(state)];
    set({
      segments: prev.segments,
      constraints: prev.constraints,
      points: prev.points,
      selectedSegments: prev.selectedSegments,
      solved: prev.solved,
      constraintStatus: prev.constraintStatus,
      history: newHistory,
      future: newFuture,
    });
  },

  redoSketch: () => {
    const state = get();
    if (state.future.length === 0) return;
    const next = state.future[state.future.length - 1]!;
    const newFuture = state.future.slice(0, -1);
    const newHistory = [...state.history, snapshot(state)];
    set({
      segments: next.segments,
      constraints: next.constraints,
      points: next.points,
      selectedSegments: next.selectedSegments,
      solved: next.solved,
      constraintStatus: next.constraintStatus,
      history: newHistory,
      future: newFuture,
    });
  },

  validateState: () => {
    const state = get();
    let fixed = false;

    // Fix: both active and faceSelectionMode true simultaneously
    if (state.active && state.faceSelectionMode) {
      set({ faceSelectionMode: false, hoveredFace: null });
      fixed = true;
    }

    // Fix: pendingExit true but active false
    if (state.pendingExit && !state.active) {
      set({ pendingExit: false });
      fixed = true;
    }

    // Fix: loftMode true but active false
    if (state.loftMode && !state.active) {
      set({ loftMode: false, profiles: [] });
      fixed = true;
    }

    return fixed;
  },

  enterFaceSelectionMode: () => {
    set({
      faceSelectionMode: true,
      hoveredFace: null,
      selectedFace: null,
    });
  },

  setHoveredFace: (face) => {
    set({ hoveredFace: face });
  },

  selectFace: (face) => {
    const plane = computePlaneFromFace(face);
    set({
      faceSelectionMode: false,
      selectedFace: face,
      hoveredFace: null,
      // Enter sketch mode with the computed plane
      active: true,
      plane,
      origin: plane.origin,
      segments: [],
      constraints: [],
      tool: "rectangle",
      constraintTool: "none",
      points: [],
      selectedSegments: [],
      solved: true,
      constraintStatus: "under",
      loftMode: false,
      profiles: [],
      pendingExit: false,
      history: [],
      future: [],
    });

    // Dispatch event to trigger camera swing to face the plane. Pass the
    // face vertices along so the camera can fit the face's actual extent
    // instead of a hardcoded distance.
    if (typeof window !== "undefined") {
      window.dispatchEvent(
        new CustomEvent("vcad:face-selected", {
          detail: {
            normal: face.normal,
            centroid: face.centroid,
            vertices: face.vertices,
          },
        })
      );
    }
  },

  cancelFaceSelection: () => {
    set({
      faceSelectionMode: false,
      hoveredFace: null,
      selectedFace: null,
    });
  },

  enterSketchMode: (plane, origin, flipped) => {
    const planeOrigin = origin ?? (typeof plane === "string" ? { x: 0, y: 0, z: 0 } : plane.origin);
    set({
      active: true,
      plane,
      origin: planeOrigin,
      segments: [],
      constraints: [],
      tool: "rectangle",
      constraintTool: "none",
      points: [],
      selectedSegments: [],
      solved: true,
      constraintStatus: "under",
      loftMode: false,
      profiles: [],
      pendingExit: false,
      history: [],
      future: [],
      faceSelectionMode: false,
      hoveredFace: null,
      selectedFace: null,
      selectedConstraintIndex: null,
      // pendingOperation deliberately NOT reset here — operations-as-entry
      // sets it before calling enterSketchMode and we want it preserved
      // through the sketch open. exitSketchMode handles the cleanup.
    });

    // Dispatch event to trigger camera swing to face the plane
    if (typeof window !== "undefined") {
      const { normal } = getSketchPlaneDirections(plane);
      // If flipped (clicked back face), negate the normal so camera swings to opposite side
      const effectiveNormal = flipped
        ? { x: -normal.x, y: -normal.y, z: -normal.z }
        : normal;
      window.dispatchEvent(
        new CustomEvent("vcad:face-selected", {
          detail: { normal: effectiveNormal, centroid: planeOrigin },
        })
      );
    }
  },

  exitSketchMode: (): SketchExitStatus => {
    const state = get();
    const hasSegments = state.segments.length > 0;
    set({
      active: false,
      points: [],
      loftMode: false,
      profiles: [],
      pendingExit: false,
      history: [],
      future: [],
      faceSelectionMode: false,
      hoveredFace: null,
      selectedFace: null,
      selectedConstraintIndex: null,
      pendingOperation: null,
    });
    return hasSegments ? "has_segments" : "empty";
  },

  requestExit: () => {
    const state = get();
    // If no segments, exit immediately
    if (state.segments.length === 0) {
      get().exitSketchMode();
      return true;
    }
    // Otherwise, show confirmation
    set({ pendingExit: true });
    return false;
  },

  confirmExit: (): SketchExitStatus => {
    set({ pendingExit: false });
    return get().exitSketchMode();
  },

  cancelExit: () => {
    set({ pendingExit: false });
  },

  setTool: (tool) => {
    set({ tool, points: [] });
  },

  addPoint: (point) => {
    const state = get();
    const newPoints = [...state.points, point];

    if (state.tool === "line") {
      if (newPoints.length >= 2) {
        // Add a line segment
        const start = newPoints[newPoints.length - 2]!;
        const end = newPoints[newPoints.length - 1]!;
        pushHistory();
        set((s) => ({
          segments: [...s.segments, { type: "Line", start, end }],
          points: [end], // Keep last point for continuation
        }));
      } else {
        set({ points: newPoints });
      }
    } else if (state.tool === "rectangle") {
      if (newPoints.length >= 2) {
        // Complete rectangle
        const p1 = newPoints[0]!;
        const p2 = newPoints[1]!;
        const rectSegments = makeRectangleSegments(p1, p2);
        pushHistory();
        set((s) => ({
          segments: [...s.segments, ...rectSegments],
          points: [],
        }));
      } else {
        set({ points: newPoints });
      }
    } else if (state.tool === "circle") {
      if (newPoints.length >= 2) {
        // Complete circle (center + edge point)
        const center = newPoints[0]!;
        const edge = newPoints[1]!;
        const radius = Math.sqrt(
          (edge.x - center.x) ** 2 + (edge.y - center.y) ** 2
        );
        if (radius > 0.1) {
          const circleSegments = makeCircleSegments(center, radius);
          pushHistory();
          set((s) => ({
            segments: [...s.segments, ...circleSegments],
            points: [],
          }));
        } else {
          set({ points: [] });
        }
      } else {
        set({ points: newPoints });
      }
    }
  },

  finishShape: () => {
    const state = get();
    if (state.tool === "line" && state.points.length > 0 && state.segments.length > 0) {
      // Close the line shape by connecting last point to first
      const firstSeg = state.segments[0];
      if (firstSeg?.type === "Line") {
        const lastPoint = state.points[0]!;
        const firstPoint = firstSeg.start;
        pushHistory();
        set((s) => ({
          segments: [...s.segments, { type: "Line", start: lastPoint, end: firstPoint }],
          points: [],
        }));
        return;
      }
    }
    set({ points: [] });
  },

  clearSketch: () => {
    if (get().segments.length === 0) return;
    pushHistory();
    set({ segments: [], points: [], constraints: [], selectedSegments: [] });
  },

  addRectangle: (p1, p2) => {
    const rectSegments = makeRectangleSegments(p1, p2);
    pushHistory();
    set((s) => ({ segments: [...s.segments, ...rectSegments] }));
  },

  addCircle: (center, radius, segments = 32) => {
    const circleSegments = makeCircleSegments(center, radius, segments);
    pushHistory();
    set((s) => ({ segments: [...s.segments, ...circleSegments] }));
  },

  // Constraint actions
  setConstraintTool: (tool) => {
    set({ constraintTool: tool, selectedSegments: [] });
  },

  toggleSegmentSelection: (index) => {
    set((s) => {
      const selected = s.selectedSegments.includes(index)
        ? s.selectedSegments.filter((i) => i !== index)
        : [...s.selectedSegments, index];
      return { selectedSegments: selected };
    });
  },

  clearSelection: () => {
    set({ selectedSegments: [], constraintTool: "none" });
  },

  addConstraint: (constraint) => {
    pushHistory();
    set((s) => {
      const newConstraints = [...s.constraints, constraint];
      return {
        constraints: newConstraints,
        solved: false,
        constraintStatus: computeConstraintStatus(s.segments.length > 0, newConstraints, false),
      };
    });
  },

  removeConstraint: (index) => {
    pushHistory();
    set((s) => {
      const newConstraints = s.constraints.filter((_, i) => i !== index);
      return {
        constraints: newConstraints,
        solved: false,
        constraintStatus: computeConstraintStatus(s.segments.length > 0, newConstraints, false),
      };
    });
  },

  solveSketch: () => {
    const state = get();
    if (state.constraints.length === 0) {
      set({ solved: true, constraintStatus: "under" });
      return;
    }

    // Delegate to the kernel's Levenberg-Marquardt solver via WASM.
    // The TUI and the web app share the same `SketchSession` implementation
    // in `vcad-kernel-constraints`, so both frontends get the same 15
    // constraint types and the same numerical behavior.
    const wasm = getKernelWasmSync() as unknown as {
      solveSketchSegments?: (segments: string, constraints: string) => string;
    } | null;

    if (!wasm || typeof wasm.solveSketchSegments !== "function") {
      // WASM not hydrated yet (e.g. during boot, or in tests) — no solve
      // actually ran, so report "pending" rather than lying with "solved",
      // and re-run automatically once the kernel module is available.
      set({ solved: false, constraintStatus: "pending" });
      getKernelWasm()
        .then(() => {
          const s = get();
          if (!s.active || s.constraintStatus !== "pending") return;
          try {
            s.solveSketch();
          } catch (err) {
            // The retry itself blew up. Surface it rather than leaving the
            // sketch parked in "pending" forever with no visible reason.
            console.warn("[sketch] solver error after hydration:", err);
            set({ solved: false, constraintStatus: "error" });
          }
        })
        .catch(() => {
          // Kernel never hydrated (headless/tests) — stay pending. Nothing
          // was solved and nothing is coming, which "pending" says honestly.
        });
      return;
    }

    try {
      const resultJson = wasm.solveSketchSegments(
        JSON.stringify(state.segments),
        JSON.stringify(state.constraints),
      );
      const result = JSON.parse(resultJson) as {
        segments: SketchSegment2D[];
        converged: boolean;
      };
      // Push *before* overwriting segments so Ctrl-Z restores the
      // pre-solve geometry — important when a user tightens constraints
      // and wants to back out of the solve's choices.
      pushHistory();
      set({
        segments: result.segments,
        solved: result.converged,
        constraintStatus: result.converged ? "solved" : "error",
      });
    } catch (err) {
      console.warn("[sketch] solver error:", err);
      set({ solved: false, constraintStatus: "error" });
    }
  },

  applyHorizontal: () => {
    const state = get();
    if (state.selectedSegments.length !== 1) return;
    const idx = state.selectedSegments[0]!;
    const seg = state.segments[idx];
    if (seg?.type !== "Line") return;

    get().addConstraint({ type: "Horizontal", line: idx });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  applyVertical: () => {
    const state = get();
    if (state.selectedSegments.length !== 1) return;
    const idx = state.selectedSegments[0]!;
    const seg = state.segments[idx];
    if (seg?.type !== "Line") return;

    get().addConstraint({ type: "Vertical", line: idx });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  applyDistance: (distance) => {
    const state = get();
    if (state.selectedSegments.length !== 2) return;
    const [a, b] = state.selectedSegments;

    get().addConstraint({
      type: "Distance",
      pointA: { type: "LineStart", index: a! },
      pointB: { type: "LineStart", index: b! },
      distance,
    });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  applyLength: (length) => {
    const state = get();
    if (state.selectedSegments.length !== 1) return;
    const idx = state.selectedSegments[0]!;

    get().addConstraint({ type: "Length", line: idx, length });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  applyParallel: () => {
    const state = get();
    if (state.selectedSegments.length !== 2) return;
    const [a, b] = state.selectedSegments;

    get().addConstraint({ type: "Parallel", lineA: a!, lineB: b! });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  applyPerpendicular: () => {
    const state = get();
    if (state.selectedSegments.length !== 2) return;
    const [a, b] = state.selectedSegments;

    get().addConstraint({ type: "Perpendicular", lineA: a!, lineB: b! });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  applyEqual: () => {
    const state = get();
    if (state.selectedSegments.length !== 2) return;
    const [a, b] = state.selectedSegments;

    get().addConstraint({ type: "EqualLength", lineA: a!, lineB: b! });
    set({ selectedSegments: [], constraintTool: "none" });
  },

  // Loft mode actions
  enterLoftMode: (plane) => {
    set({
      active: true,
      plane,
      origin: { x: 0, y: 0, z: 0 },
      segments: [],
      constraints: [],
      tool: "rectangle",
      constraintTool: "none",
      points: [],
      selectedSegments: [],
      solved: true,
      constraintStatus: "under",
      loftMode: true,
      profiles: [],
    });
  },

  saveProfile: () => {
    const state = get();
    if (state.segments.length === 0) return;

    const profile: ProfileSnapshot = {
      id: `profile-${++profileIdCounter}`,
      plane: state.plane,
      origin: state.origin,
      segments: [...state.segments],
    };

    set({
      profiles: [...state.profiles, profile],
    });
  },

  clearForNextProfile: (newOrigin) => {
    set({
      segments: [],
      constraints: [],
      points: [],
      selectedSegments: [],
      solved: true,
      constraintStatus: "under",
      origin: newOrigin,
    });
  },

  exitLoftMode: () => {
    const state = get();
    if (!state.loftMode) return null;

    // If there are unsaved segments, save them as the last profile
    let allProfiles = [...state.profiles];
    if (state.segments.length > 0) {
      allProfiles.push({
        id: `profile-${++profileIdCounter}`,
        plane: state.plane,
        origin: state.origin,
        segments: [...state.segments],
      });
    }

    set({
      active: false,
      loftMode: false,
      profiles: [],
      segments: [],
      points: [],
    });

    // Return profiles only if we have at least 2
    return allProfiles.length >= 2 ? allProfiles : null;
  },

  setCursorPos: (world, sketch, snap) => {
    set({ cursorWorldPos: world, cursorSketchPos: sketch, snapTarget: snap });
  },

  setSelectedConstraint: (index) => {
    set({ selectedConstraintIndex: index });
  },

  beginOperation: (op) => {
    // Replace any prior pending op outright — switching from Extrude to
    // Revolve mid-sketch should swap params cleanly. Caller is responsible
    // for picking the right kind; we trust the discriminated union.
    set({ pendingOperation: op });
  },

  updateOperation: (patch) => {
    const current = get().pendingOperation;
    if (!current) return;
    // Patch must match the current discriminant — TS keeps callers honest;
    // at runtime we fall back to silently ignoring kind mismatches so a
    // stale slider event can't corrupt state when the user has just
    // switched ops.
    if ("kind" in patch && patch.kind !== current.kind) return;
    set({ pendingOperation: { ...current, ...patch } as PendingOperation });
  },

  clearOperation: () => {
    set({ pendingOperation: null });
  },
  };
});

/**
 * Default params for each pending operation kind. UI calls
 * `beginOperation(defaultPendingOperation('extrude'))` rather than
 * literal-coding the params at every call site.
 */
export function defaultPendingOperation<K extends PendingOperation["kind"]>(
  kind: K,
): Extract<PendingOperation, { kind: K }> {
  return DEFAULT_PENDING_OPERATION[kind] as Extract<PendingOperation, { kind: K }>;
}
