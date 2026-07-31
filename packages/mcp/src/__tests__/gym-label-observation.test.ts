/**
 * Unit tests for the gym observation labeling.
 *
 * A joint owns a *slice* of the observation arrays, sized `max(1, ndof)`:
 * Fixed 1, Revolute / Slider / Cylindrical 1, Ball 3, Free 6. Labeling that
 * assumed one slot per joint silently dropped the entire `joints` view for
 * any env holding a Ball or Free joint — the bare arrays stayed correct, so
 * nothing errored; callers just got `undefined` where they expected the
 * id-keyed view.
 *
 * These exercise the pure function directly so they hold regardless of which
 * kernel WASM is loaded (the checked-in artifact predates the Free joint).
 */

import { describe, it, expect } from "vitest";
import { labelObservation } from "../tools/gym.js";
import type { PhysicsObservation } from "@vcad/engine";

const obs = (positions: number[], velocities: number[]): PhysicsObservation => ({
  joint_positions: positions,
  joint_velocities: velocities,
  end_effector_poses: [],
});

describe("labelObservation", () => {
  it("labels single-DOF joints one slot each", () => {
    const out = labelObservation(
      obs([10, 20], [1, 2]),
      ["a", "b"],
      [],
      [1, 1],
    );
    expect(out.joints).toEqual([
      { id: "a", position: 10, velocity: 1 },
      { id: "b", position: 20, velocity: 2 },
    ]);
  });

  it("gives a Free joint all six of its slots", () => {
    const positions = [1, 2, 3, 4, 5, 6];
    const velocities = [7, 8, 9, 10, 11, 12];
    const out = labelObservation(obs(positions, velocities), ["base"], [], [6]);

    expect(out.joints).toHaveLength(1);
    const joint = out.joints![0];
    expect(joint.id).toBe("base");
    expect(joint.positions).toEqual(positions);
    expect(joint.velocities).toEqual(velocities);
    // The scalar stays the first slot, so single-DOF callers are unaffected.
    expect(joint.position).toBe(1);
    expect(joint.velocity).toBe(7);
  });

  it("splits a mixed Revolute + Ball + Free env at the right boundaries", () => {
    // 1 + 3 + 6 = 10 slots across three joints.
    const positions = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    const velocities = positions.map((n) => n * 10);
    const out = labelObservation(
      obs(positions, velocities),
      ["elbow", "wrist", "base"],
      [],
      [1, 3, 6],
    );

    expect(out.joints).toHaveLength(3);
    const [elbow, wrist, base] = out.joints!;

    // A single-DOF joint keeps the bare scalar and no array.
    expect(elbow.position).toBe(0);
    expect(elbow.positions).toBeUndefined();

    expect(wrist.positions).toEqual([1, 2, 3]);
    expect(wrist.velocities).toEqual([10, 20, 30]);

    expect(base.positions).toEqual([4, 5, 6, 7, 8, 9]);
    expect(base.velocities).toEqual([40, 50, 60, 70, 80, 90]);
  });

  it("omits the view when slot counts don't tile the observation", () => {
    // Metadata disagrees with the data — labeling anything would
    // mis-attribute values, so emit nothing rather than a partial view.
    const out = labelObservation(obs([1, 2, 3], [1, 2, 3]), ["a"], [], [6]);
    expect(out.joints).toBeUndefined();
    expect(out.joint_ids).toEqual(["a"]);
  });

  it("falls back to one slot per joint when the kernel reports no counts", () => {
    const out = labelObservation(obs([5], [6]), ["a"], [], null);
    expect(out.joints).toEqual([{ id: "a", position: 5, velocity: 6 }]);
  });

  it("omits the view entirely when joint ids are unavailable", () => {
    const out = labelObservation(obs([1], [2]), null, [], null);
    expect(out.joints).toBeUndefined();
    expect(out.joint_ids).toBeNull();
  });
});
