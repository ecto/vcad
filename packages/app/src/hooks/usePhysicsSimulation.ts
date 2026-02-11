/**
 * Hook for running physics simulation in the viewport.
 *
 * Initializes the physics environment when an assembly with joints is loaded,
 * runs the simulation loop based on the simulation store's mode,
 * and updates instance transforms from physics observations.
 */

import { useEffect, useRef, useCallback } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import {
  PhysicsEnv,
  isPhysicsAvailable,
  solveForwardKinematics,
  type PhysicsObservation,
} from "@vcad/engine";
import {
  useDocumentStore,
  useEngineStore,
  useSimulationStore,
  type JointState,
} from "@vcad/core";

/**
 * Hook to manage physics simulation lifecycle.
 *
 * Call this in a component within the R3F Canvas tree.
 */
export function usePhysicsSimulation() {
  const envRef = useRef<PhysicsEnv | null>(null);
  const accumulatorRef = useRef<number>(0);
  const jointIdsRef = useRef<string[]>([]);

  // Document state - use stable selectors to avoid recreating on every change
  const joints = useDocumentStore((s) => s.document.joints);
  const setJointState = useDocumentStore((s) => s.setJointState);

  // Compute a stable key based on joint IDs (structure, not state)
  const jointStructureKey = joints?.map((j) => j.id).join(",") ?? "";

  // Simulation state
  const mode = useSimulationStore((s) => s.mode);
  const timestep = useSimulationStore((s) => s.timestep);
  const playbackSpeed = useSimulationStore((s) => s.playbackSpeed);
  const setPhysicsAvailable = useSimulationStore((s) => s.setPhysicsAvailable);
  const setJointStates = useSimulationStore((s) => s.setJointStates);
  const setObservation = useSimulationStore((s) => s.setObservation);
  const setMode = useSimulationStore((s) => s.setMode);

  // Check physics availability on mount
  useEffect(() => {
    isPhysicsAvailable().then((available) => {
      console.log("[Physics] isPhysicsAvailable:", available);
      setPhysicsAvailable(available);
    });
  }, [setPhysicsAvailable]);

  // Initialize physics environment when joint structure changes
  useEffect(() => {
    if (!joints || joints.length === 0) {
      // No joints, clean up any existing env
      if (envRef.current) {
        envRef.current.close();
        envRef.current = null;
        jointIdsRef.current = [];
      }
      return;
    }

    // Create physics environment
    let mounted = true;

    async function initPhysics() {
      const available = await isPhysicsAvailable();
      if (!available || !mounted) return;

      try {
        // Get current document snapshot for physics initialization
        const doc = useDocumentStore.getState().document;

        // Get end effector IDs (for now, empty - could be leaf instances)
        const endEffectorIds: string[] = [];

        const env = await PhysicsEnv.create(doc, {
          endEffectorIds,
          dt: 1 / 240,
          substeps: 4,
        });

        if (!mounted) {
          env.close();
          return;
        }

        // Close previous env if exists
        if (envRef.current) {
          envRef.current.close();
        }

        envRef.current = env;

        // Get joints from current state (already validated as non-empty)
        const currentJoints = useDocumentStore.getState().document.joints ?? [];
        jointIdsRef.current = currentJoints.map((j) => j.id);

        // Initialize joint states from document
        const initialStates: JointState[] = currentJoints.map((joint) => ({
          id: joint.id,
          name: joint.name ?? joint.id,
          position: joint.state ?? 0,
          velocity: 0,
          torque: 0,
          limits: null, // Joint limits could be derived from JointKind in future
        }));
        setJointStates(initialStates);

        // Get initial observation
        const obs = env.observe();
        setObservation({
          jointPositions: obs.joint_positions,
          jointVelocities: obs.joint_velocities,
          endEffectorPoses: obs.end_effector_poses,
        });

        console.log(
          `[Physics] Initialized with ${env.numJoints} joints, ${env.actionDim} action dim`
        );
      } catch (err) {
        console.error("[Physics] Failed to initialize:", err);
      }
    }

    initPhysics();

    return () => {
      mounted = false;
    };
  }, [jointStructureKey, setJointStates, setObservation]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (envRef.current) {
        envRef.current.close();
        envRef.current = null;
      }
    };
  }, []);

  // Convert joint state positions to action targets
  const getJointTargets = useCallback((): number[] => {
    // Get current state directly from store to avoid dependency on jointStates
    const currentStates = useSimulationStore.getState().jointStates;
    return currentStates.map((js) => js.position);
  }, []);

  // Get joint torques for torque control mode
  const getJointTorques = useCallback((): number[] => {
    const currentStates = useSimulationStore.getState().jointStates;
    return currentStates.map((js) => js.torque);
  }, []);

  // Update scene transforms directly from physics observation (bypasses CSG re-eval)
  const updateFromObservation = useCallback(
    (obs: PhysicsObservation) => {
      // Update simulation store for UI display
      setObservation({
        jointPositions: obs.joint_positions,
        jointVelocities: obs.joint_velocities,
        endEffectorPoses: obs.end_effector_poses,
      });

      // Update simulation store's joint states
      const currentStates = useSimulationStore.getState().jointStates;
      const newStates = currentStates.map((js, i) => ({
        ...js,
        position: obs.joint_positions[i] ?? js.position,
        velocity: obs.joint_velocities[i] ?? js.velocity,
      }));
      setJointStates(newStates);

      // Create a temporary document with updated joint states for FK calculation
      // Don't update the actual document to avoid triggering re-evaluation
      const doc = useDocumentStore.getState().document;
      const tempDoc = structuredClone(doc);
      jointIdsRef.current.forEach((jointId, i) => {
        const joint = tempDoc.joints?.find((j) => j.id === jointId);
        if (joint && obs.joint_positions[i] !== undefined) {
          joint.state = obs.joint_positions[i];
        }
      });

      // Compute FK and update scene transforms directly (no CSG re-eval)
      const worldTransforms = solveForwardKinematics(tempDoc);
      useEngineStore.getState().updateInstanceTransforms(worldTransforms);
    },
    [setObservation, setJointStates]
  );

  const { invalidate } = useThree();

  // Simulation loop using useFrame
  useFrame((_, delta) => {
    const env = envRef.current;
    if (!env) return;

    // Handle different simulation modes
    if (mode === "off" || mode === "paused") {
      return;
    }

    if (mode === "stepping") {
      // Single step mode - step once then pause
      const actionType = useSimulationStore.getState().actionType;
      const actions = actionType === "torque" ? getJointTorques() : getJointTargets();
      const result = env.step(actionType, actions);
      updateFromObservation(result.observation);
      setMode("paused");
      return;
    }

    if (mode === "running") {
      // Fixed timestep accumulator for consistent physics
      const scaledDelta = delta * playbackSpeed;
      accumulatorRef.current += scaledDelta;

      // Get action type once per frame
      const actionType = useSimulationStore.getState().actionType;

      // Step physics at fixed timestep
      while (accumulatorRef.current >= timestep) {
        const actions = actionType === "torque" ? getJointTorques() : getJointTargets();
        const result = env.step(actionType, actions);

        // Debug: log physics state occasionally
        if (Math.random() < 0.01) {
          console.log("[Physics] actionType:", actionType, "actions:", actions, "positions:", result.observation.joint_positions);
        }

        updateFromObservation(result.observation);

        // Note: result.done is for RL training episodes (max_steps reached).
        // For interactive sessions, we continue running - user controls playback.
        // Uncomment below to auto-pause on episode end:
        // if (result.done) { setMode("paused"); break; }

        accumulatorRef.current -= timestep;
      }

      // Keep requesting frames while simulation is running (demand mode)
      invalidate();
    }
  });

  // Handle stop/reset - sync final joint states to document for persistence
  useEffect(() => {
    if (mode === "off" && envRef.current) {
      // Sync final joint states to document before reset
      const finalStates = useSimulationStore.getState().jointStates;
      jointIdsRef.current.forEach((jointId, i) => {
        const finalPos = finalStates[i]?.position;
        if (finalPos !== undefined) {
          setJointState(jointId, finalPos, true); // skipUndo
        }
      });

      // Reset physics to initial state
      const obs = envRef.current.reset();
      updateFromObservation(obs);
      accumulatorRef.current = 0;
    }
  }, [mode, updateFromObservation, setJointState]);

  return {
    isInitialized: !!envRef.current,
    numJoints: envRef.current?.numJoints ?? 0,
  };
}
