/**
 * Simulation store for physics simulation state.
 *
 * Manages the state for interactive physics simulation of robot assemblies.
 */

import { create } from "zustand";

/** Simulation playback mode */
export type SimulationMode = "off" | "paused" | "running" | "stepping";

/** Action type for joint control */
export type ActionType = "position" | "velocity" | "torque";

/** Joint state from physics simulation */
export interface JointState {
  id: string;
  name: string;
  position: number; // degrees for revolute, mm for prismatic
  velocity: number; // deg/s or mm/s
  torque: number; // Nm or N
  /** Motor drive target velocity (deg/s or mm/s) used in "velocity" action mode. */
  targetVelocity: number;
  limits: [number, number] | null;
}

/** Observation from simulation */
export interface SimulationObservation {
  jointPositions: number[];
  jointVelocities: number[];
  endEffectorPoses: Array<[number, number, number, number, number, number, number]>;
}

/** Simulation state store */
export interface SimulationState {
  /** Current simulation mode */
  mode: SimulationMode;
  /** Simulation timestep in seconds */
  timestep: number;
  /** Playback speed multiplier (1.0 = realtime) */
  playbackSpeed: number;
  /** Whether physics is available */
  physicsAvailable: boolean;
  /** Current simulation step count */
  stepCount: number;
  /** Joint states */
  jointStates: JointState[];
  /** Selected joint ID for control */
  selectedJointId: string | null;
  /** Action type for control inputs */
  actionType: ActionType;
  /** End effector instance IDs to track */
  endEffectorIds: string[];
  /** Last observation from simulation */
  lastObservation: SimulationObservation | null;

  // Actions
  setMode: (mode: SimulationMode) => void;
  play: () => void;
  pause: () => void;
  stop: () => void;
  step: () => void;
  setTimestep: (dt: number) => void;
  setPlaybackSpeed: (speed: number) => void;
  setPhysicsAvailable: (available: boolean) => void;
  setJointStates: (states: JointState[]) => void;
  updateJointState: (id: string, position: number, velocity: number) => void;
  /** Set a joint's motor drive target velocity (deg/s or mm/s). */
  setJointTargetVelocity: (id: string, targetVelocity: number) => void;
  selectJoint: (id: string | null) => void;
  setActionType: (type: ActionType) => void;
  setEndEffectorIds: (ids: string[]) => void;
  setObservation: (obs: SimulationObservation) => void;
  reset: () => void;
}

export const useSimulationStore = create<SimulationState>((set) => ({
  mode: "off",
  timestep: 1 / 60,
  playbackSpeed: 1.0,
  physicsAvailable: false,
  stepCount: 0,
  jointStates: [],
  selectedJointId: null,
  actionType: "torque",
  endEffectorIds: [],
  lastObservation: null,

  setMode: (mode) => set({ mode }),

  play: () =>
    set((s) => ({
      mode: s.physicsAvailable ? "running" : "off",
    })),

  pause: () =>
    set((s) => ({
      mode: s.mode === "running" ? "paused" : s.mode,
    })),

  stop: () =>
    set({
      mode: "off",
      stepCount: 0,
      lastObservation: null,
    }),

  step: () =>
    set((s) => ({
      mode: "stepping",
      stepCount: s.stepCount + 1,
    })),

  setTimestep: (timestep) => set({ timestep }),

  setPlaybackSpeed: (playbackSpeed) => set({ playbackSpeed }),

  setPhysicsAvailable: (physicsAvailable) => set({ physicsAvailable }),

  setJointStates: (jointStates) => set({ jointStates }),

  updateJointState: (id, position, velocity) =>
    set((s) => ({
      jointStates: s.jointStates.map((js) =>
        js.id === id ? { ...js, position, velocity } : js
      ),
    })),

  setJointTargetVelocity: (id, targetVelocity) =>
    set((s) => ({
      jointStates: s.jointStates.map((js) =>
        js.id === id ? { ...js, targetVelocity } : js
      ),
      // Driving a motor implies velocity control for the whole env.
      actionType: "velocity",
    })),

  selectJoint: (selectedJointId) => set({ selectedJointId }),

  setActionType: (actionType) => set({ actionType }),

  setEndEffectorIds: (endEffectorIds) => set({ endEffectorIds }),

  setObservation: (lastObservation) =>
    set((s) => ({
      lastObservation,
      stepCount: s.stepCount + 1,
    })),

  reset: () =>
    set({
      mode: "off",
      stepCount: 0,
      jointStates: [],
      selectedJointId: null,
      lastObservation: null,
    }),
}));
