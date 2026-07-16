import { describe, expect, it } from "vitest";
import type { AnimTrack, Document, Timeline } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import {
  poseDocument,
  sampleSequence,
  sampleTrackValue,
} from "../sequence.js";

function track(target: AnimTrack["target"], keys: AnimTrack["keys"]): AnimTrack {
  return { target, keys };
}

describe("sampleTrackValue", () => {
  it("interpolates linearly and clamps outside the key range", () => {
    const tr = track({ type: "Explode" }, [
      { t: 0, value: 0 },
      { t: 2, value: 10 },
    ]);
    expect(sampleTrackValue(tr, -1)).toBe(0);
    expect(sampleTrackValue(tr, 1)).toBe(5);
    expect(sampleTrackValue(tr, 3)).toBe(10);
  });

  it("step holds the previous value until the destination key", () => {
    const tr = track({ type: "Explode" }, [
      { t: 0, value: 1 },
      { t: 1, value: 2, ease: "step" },
    ]);
    expect(sampleTrackValue(tr, 0.5)).toBe(1);
    expect(sampleTrackValue(tr, 0.999)).toBe(1);
    expect(sampleTrackValue(tr, 1)).toBe(2);
  });

  it("ease-in-out is smoothstep (0.5 at midpoint, lags linear early)", () => {
    const tr = track({ type: "Explode" }, [
      { t: 0, value: 0 },
      { t: 1, value: 1, ease: "ease-in-out" },
    ]);
    expect(sampleTrackValue(tr, 0.5)).toBe(0.5);
    const quarter = sampleTrackValue(tr, 0.25);
    expect(quarter).toBeCloseTo(0.25 * 0.25 * (3 - 2 * 0.25), 12);
    expect(quarter).toBeLessThan(0.25);
  });
});

function docWithTimeline(timeline: Timeline): Document {
  const doc = createDocument();
  doc.timeline = timeline;
  return doc;
}

describe("sampleSequence", () => {
  it("frame count is round(durationS*fps)+1 inclusive of t=0", () => {
    const frames = sampleSequence(
      docWithTimeline({ durationS: 2, fps: 24, tracks: [], camera: [] }),
    );
    expect(frames.length).toBe(49);
    expect(frames[0].t).toBe(0);
    expect(frames[48].t).toBeCloseTo(2, 12);
  });

  it("defaults fps to 24 and clamps to at least 2 frames", () => {
    const frames = sampleSequence(
      docWithTimeline({ durationS: 1, fps: 0, tracks: [], camera: [] }),
    );
    expect(frames.length).toBe(25);
    const tiny = sampleSequence(
      docWithTimeline({ durationS: 0, fps: 24, tracks: [], camera: [] }),
    );
    expect(tiny.length).toBe(2);
  });

  it("samples params/joints/visibility/explode and flags geometryDirty", () => {
    const frames = sampleSequence(
      docWithTimeline({
        durationS: 1,
        fps: 4,
        tracks: [
          track({ type: "Parameter", name: "width" }, [
            { t: 0, value: 10 },
            { t: 1, value: 20 },
          ]),
          track({ type: "Joint", jointId: "j1" }, [
            { t: 0, value: 0 },
            { t: 1, value: 90 },
          ]),
          track({ type: "Visibility", instanceId: "lid" }, [
            { t: 0, value: 1 },
            { t: 0.5, value: 0, ease: "step" },
          ]),
          track({ type: "Explode" }, [
            { t: 0, value: 0 },
            { t: 1, value: 1 },
          ]),
        ],
        camera: [],
      }),
    );
    expect(frames.length).toBe(5);
    expect(frames[0].params.width).toBe(10);
    expect(frames[2].params.width).toBe(15);
    expect(frames[4].joints.j1).toBe(90);
    expect(frames[0].visibility.lid).toBe(true);
    expect(frames[3].visibility.lid).toBe(false);
    expect(frames[2].explode).toBe(0.5);
    // param track present → frame 0 dirty; changing values keep it dirty
    expect(frames.every((f) => f.geometryDirty)).toBe(true);
  });

  it("geometryDirty is false when params are static", () => {
    const frames = sampleSequence(
      docWithTimeline({
        durationS: 1,
        fps: 2,
        tracks: [
          track({ type: "Parameter", name: "width" }, [{ t: 0, value: 10 }]),
        ],
        camera: [],
      }),
    );
    expect(frames[0].geometryDirty).toBe(true);
    expect(frames[1].geometryDirty).toBe(false);
    const none = sampleSequence(
      docWithTimeline({ durationS: 1, fps: 2, tracks: [], camera: [] }),
    );
    expect(none[0].geometryDirty).toBe(false);
  });

  it("turntable sweeps azimuth over the shot; default pose in gaps", () => {
    const frames = sampleSequence(
      docWithTimeline({
        durationS: 2,
        fps: 2,
        tracks: [],
        camera: [
          {
            startS: 0,
            endS: 2,
            kind: { type: "Turntable", degrees: 360, elevationDeg: 45 },
          },
        ],
      }),
    );
    expect(frames[0].camera).toEqual({
      azimuthDeg: 0,
      elevationDeg: 45,
      dolly: 1,
    });
    expect(frames[2].camera.azimuthDeg).toBeCloseTo(180, 12);
    expect(frames[2].camera.elevationDeg).toBe(45);
    // t=2 is outside [0,2) → holds previous pose
    expect(frames[4].camera.azimuthDeg).toBeCloseTo(
      frames[3].camera.azimuthDeg,
      12,
    );
  });

  it("defaults to {0, 30, 1} with no camera shots", () => {
    const frames = sampleSequence(
      docWithTimeline({ durationS: 1, fps: 2, tracks: [], camera: [] }),
    );
    expect(frames[0].camera).toEqual({
      azimuthDeg: 0,
      elevationDeg: 30,
      dolly: 1,
    });
  });

  it("focus holds prior azimuth/elevation and dollys toward target", () => {
    const frames = sampleSequence(
      docWithTimeline({
        durationS: 2,
        fps: 2,
        tracks: [],
        camera: [
          {
            startS: 0,
            endS: 1,
            kind: { type: "Orbit", from: [0, 10], to: [90, 40] },
          },
          {
            startS: 1,
            endS: 2,
            kind: { type: "Focus", target: "lid", dolly: 0.5 },
          },
        ],
      }),
    );
    // last orbit sample (t=0.5) → az 45, el 25
    expect(frames[1].camera.azimuthDeg).toBeCloseTo(45, 12);
    // focus at t=1.5, u=0.5 → dolly 0.75, holds previous az/el
    expect(frames[3].camera.dolly).toBeCloseTo(0.75, 12);
    expect(frames[3].camera.target).toBe("lid");
    expect(frames[3].camera.azimuthDeg).toBe(frames[1].camera.azimuthDeg);
  });

  it("returns [] without a timeline; timelineOverride wins", () => {
    const doc = createDocument();
    expect(sampleSequence(doc)).toEqual([]);
    const frames = sampleSequence(doc, {
      durationS: 1,
      fps: 1,
      tracks: [],
      camera: [],
    });
    expect(frames.length).toBe(2);
  });
});

describe("poseDocument", () => {
  it("applies params and joint states to a clone without mutating the original", () => {
    const doc = createDocument();
    doc.parameters = {
      width: { value: 10 },
      untouched: { value: 3 },
    };
    doc.joints = [
      {
        id: "j1",
        parentInstanceId: null,
        childInstanceId: "a",
        parentAnchor: { x: 0, y: 0, z: 0 },
        childAnchor: { x: 0, y: 0, z: 0 },
        kind: { type: "Revolute", axis: { x: 0, y: 0, z: 1 } },
        state: 0,
      },
    ];
    const frames = sampleSequence(doc, {
      durationS: 1,
      fps: 1,
      tracks: [
        track({ type: "Parameter", name: "width" }, [
          { t: 0, value: 10 },
          { t: 1, value: 20 },
        ]),
        track({ type: "Parameter", name: "missing" }, [{ t: 0, value: 5 }]),
        track({ type: "Joint", jointId: "j1" }, [
          { t: 0, value: 0 },
          { t: 1, value: 90 },
        ]),
      ],
      camera: [],
    });
    const posed = poseDocument(doc, frames[1]);
    expect(posed.parameters?.width.value).toBe(20);
    expect(posed.parameters?.untouched.value).toBe(3);
    expect(posed.parameters?.missing).toBeUndefined();
    expect(posed.joints?.[0].state).toBe(90);
    // original untouched
    expect(doc.parameters.width.value).toBe(10);
    expect(doc.joints[0].state).toBe(0);
    expect(posed).not.toBe(doc);
  });
});
