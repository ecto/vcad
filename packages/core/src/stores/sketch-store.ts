import { create } from "zustand";
import type { Vec2, Vec3, SketchSegment2D, SketchConstraint } from "@vcad/ir";
import type { SketchPlane, SketchState, ConstraintTool, ConstraintStatus, FaceInfo } from "../types.js";
import { computePlaneFromFace, getSketchPlaneDirections } from "../types.js";

/** A saved profile snapshot for loft operations */
export interface ProfileSnapshot {
  id: string;
  plane: SketchPlane;
  origin: Vec3;
  segments: SketchSegment2D[];
}

export type SketchExitStatus = "cancelled" | "empty" | "has_segments";

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
}

function makeRectangleSegments(p1: Vec2, p2: Vec2): SketchSegment2D[] {
  const minX = Math.min(p1.x, p2.x);
  const maxX = Math.max(p1.x, p2.x);
  const minY = Math.min(p1.y, p2.y);
  const maxY = Math.max(p1.y, p2.y);

  return [
    { type: "Line", start: { x: minX, y: minY }, end: { x: maxX, y: minY } },
    { type: "Line", start: { x: maxX, y: minY }, end: { x: maxX, y: maxY } },
    { type: "Line", start: { x: maxX, y: maxY }, end: { x: minX, y: maxY } },
    { type: "Line", start: { x: minX, y: maxY }, end: { x: minX, y: minY } },
  ];
}

function makeCircleSegments(center: Vec2, radius: number, n: number = 32): SketchSegment2D[] {
  const segments: SketchSegment2D[] = [];
  for (let i = 0; i < n; i++) {
    const theta0 = (2 * Math.PI * i) / n;
    const theta1 = (2 * Math.PI * (i + 1)) / n;
    segments.push({
      type: "Arc",
      start: {
        x: center.x + radius * Math.cos(theta0),
        y: center.y + radius * Math.sin(theta0),
      },
      end: {
        x: center.x + radius * Math.cos(theta1),
        y: center.y + radius * Math.sin(theta1),
      },
      center,
      ccw: true,
    });
  }
  return segments;
}

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

    // Dispatch event to trigger camera swing to face the plane
    if (typeof window !== "undefined") {
      window.dispatchEvent(
        new CustomEvent("vcad:face-selected", {
          detail: { normal: face.normal, centroid: face.centroid },
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

    // Simple constraint solver for common cases
    const newSegments = [...state.segments];
    let changed = false;

    for (const constraint of state.constraints) {
      if (constraint.type === "Horizontal") {
        const seg = newSegments[constraint.line];
        if (seg?.type === "Line") {
          const midY = (seg.start.y + seg.end.y) / 2;
          newSegments[constraint.line] = {
            ...seg,
            start: { ...seg.start, y: midY },
            end: { ...seg.end, y: midY },
          };
          changed = true;
        }
      } else if (constraint.type === "Vertical") {
        const seg = newSegments[constraint.line];
        if (seg?.type === "Line") {
          const midX = (seg.start.x + seg.end.x) / 2;
          newSegments[constraint.line] = {
            ...seg,
            start: { ...seg.start, x: midX },
            end: { ...seg.end, x: midX },
          };
          changed = true;
        }
      } else if (constraint.type === "Length") {
        const seg = newSegments[constraint.line];
        if (seg?.type === "Line") {
          const dx = seg.end.x - seg.start.x;
          const dy = seg.end.y - seg.start.y;
          const currentLen = Math.sqrt(dx * dx + dy * dy);
          if (currentLen > 0.001) {
            const scale = constraint.length / currentLen;
            const mx = (seg.start.x + seg.end.x) / 2;
            const my = (seg.start.y + seg.end.y) / 2;
            newSegments[constraint.line] = {
              ...seg,
              start: { x: mx - (dx * scale) / 2, y: my - (dy * scale) / 2 },
              end: { x: mx + (dx * scale) / 2, y: my + (dy * scale) / 2 },
            };
            changed = true;
          }
        }
      } else if (constraint.type === "Fixed") {
        // Find the segment containing the point
        const ref = constraint.point;
        if (ref.type === "LineStart" || ref.type === "LineEnd") {
          const seg = newSegments[ref.index];
          if (seg?.type === "Line") {
            if (ref.type === "LineStart") {
              const dx = constraint.x - seg.start.x;
              const dy = constraint.y - seg.start.y;
              newSegments[ref.index] = {
                ...seg,
                start: { x: constraint.x, y: constraint.y },
                end: { x: seg.end.x + dx, y: seg.end.y + dy },
              };
            } else {
              const dx = constraint.x - seg.end.x;
              const dy = constraint.y - seg.end.y;
              newSegments[ref.index] = {
                ...seg,
                start: { x: seg.start.x + dx, y: seg.start.y + dy },
                end: { x: constraint.x, y: constraint.y },
              };
            }
            changed = true;
          }
        }
      }
    }

    if (changed) {
      set({ segments: newSegments, solved: true, constraintStatus: "solved" });
    } else {
      set({ solved: true, constraintStatus: "solved" });
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
  };
});
