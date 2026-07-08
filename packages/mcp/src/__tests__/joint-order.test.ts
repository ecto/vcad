import { describe, expect, it } from "vitest";
import { resolveObservationJoints } from "../tools/joint-order.js";

const joint = (id: string, state = 0) =>
  ({ id, state }) as unknown as Parameters<
    typeof resolveObservationJoints
  >[0][number];

describe("resolveObservationJoints", () => {
  it("aligns doc joints to the env's joint-id order", () => {
    const docJoints = [joint("a"), joint("b"), joint("c")];
    const result = resolveObservationJoints(docJoints, ["c", "a", "b"]);
    if ("error" in result) throw new Error(result.error);
    expect(result.joints.map((j) => j.id)).toEqual(["c", "a", "b"]);
    // Same objects, not copies — writes must land on the doc's joints.
    expect(result.joints[1]).toBe(docJoints[0]);
  });

  it("falls back to positional order when the kernel exposes no ids", () => {
    const docJoints = [joint("a"), joint("b")];
    const result = resolveObservationJoints(docJoints, null);
    if ("error" in result) throw new Error(result.error);
    expect(result.joints).toBe(docJoints);
  });

  it("names env joints missing from the document", () => {
    const result = resolveObservationJoints([joint("a")], ["a", "ghost"]);
    expect(result).toHaveProperty("error");
    if (!("error" in result)) throw new Error("expected error");
    expect(result.error).toContain("ghost");
  });
});
