/**
 * Drives document-timeline playback inside the R3F canvas.
 *
 * Samples joint tracks at the playhead, runs FK on a temp document clone,
 * and pushes world transforms through the engine store — the same transient
 * path as the physics loop, so playback never mutates the document or
 * triggers CSG re-evaluation.
 *
 * Yields to the physics simulation: while the sim is running/stepping the
 * playhead freezes (two systems must not fight over instance transforms).
 */

import { useEffect, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import { solveForwardKinematics } from "@vcad/engine";
import {
  useDocumentStore,
  useEngineStore,
  useSimulationStore,
} from "@vcad/core";
import { useAnimationStore } from "@/stores/animation-store";
import { sampleJointTracks } from "@/lib/timeline-sample";

export function useTimelinePlayback() {
  const lastAppliedRef = useRef<number | null>(null);
  const { invalidate } = useThree();

  // Demand-rendered canvas: useFrame only fires when something invalidates.
  // Wake it on any transport change so scrubs/paused seeks apply immediately
  // and pressing play restarts the frame loop.
  useEffect(
    () =>
      useAnimationStore.subscribe(() => {
        invalidate();
      }),
    [invalidate],
  );

  useFrame((_, delta) => {
    const timeline = useDocumentStore.getState().document.timeline;
    if (!timeline || (timeline.tracks?.length ?? 0) === 0) return;

    const anim = useAnimationStore.getState();
    if (!anim.visible) {
      lastAppliedRef.current = null;
      return;
    }

    const simMode = useSimulationStore.getState().mode;
    if (simMode === "running" || simMode === "stepping") return;

    const duration = Math.max(timeline.durationS, 1e-6);
    let t = anim.timeS;

    if (anim.playing) {
      t += delta * anim.speed;
      if (t >= duration) {
        if (anim.loop) {
          t %= duration;
        } else {
          t = duration;
          useAnimationStore.setState({ playing: false });
        }
      }
      useAnimationStore.setState({ timeS: t });
    } else {
      t = Math.min(Math.max(t, 0), duration);
    }

    // Apply only when the playhead actually moved (scrub or playback).
    if (lastAppliedRef.current === t) return;
    lastAppliedRef.current = t;

    const jointValues = sampleJointTracks(timeline, t);
    if (jointValues.size === 0) return;

    const doc = useDocumentStore.getState().document;
    const tempDoc = structuredClone(doc);
    let touched = false;
    for (const joint of tempDoc.joints ?? []) {
      const v = jointValues.get(joint.id);
      if (v !== undefined) {
        joint.state = v;
        touched = true;
      }
    }
    if (!touched) return;

    const worldTransforms = solveForwardKinematics(tempDoc);
    useEngineStore.getState().updateInstanceTransforms(worldTransforms);
    invalidate();
  });
}
