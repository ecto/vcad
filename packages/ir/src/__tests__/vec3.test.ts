import { describe, it, expect } from "vitest";
import {
  vec3Add,
  vec3Sub,
  vec3Scale,
  vec3Dot,
  vec3Cross,
  vec3Length,
  vec3Normalize,
  vec3Negate,
  vec3Zero,
} from "../vec3.js";

describe("vec3 utilities", () => {
  it("vec3Add", () => {
    expect(vec3Add({ x: 1, y: 2, z: 3 }, { x: 4, y: 5, z: 6 })).toEqual({
      x: 5,
      y: 7,
      z: 9,
    });
  });

  it("vec3Sub", () => {
    expect(vec3Sub({ x: 5, y: 7, z: 9 }, { x: 4, y: 5, z: 6 })).toEqual({
      x: 1,
      y: 2,
      z: 3,
    });
  });

  it("vec3Scale", () => {
    expect(vec3Scale({ x: 1, y: 2, z: 3 }, 2)).toEqual({
      x: 2,
      y: 4,
      z: 6,
    });
  });

  it("vec3Dot", () => {
    expect(vec3Dot({ x: 1, y: 0, z: 0 }, { x: 0, y: 1, z: 0 })).toBe(0);
    expect(vec3Dot({ x: 1, y: 2, z: 3 }, { x: 4, y: 5, z: 6 })).toBe(32);
  });

  it("vec3Cross", () => {
    const result = vec3Cross({ x: 1, y: 0, z: 0 }, { x: 0, y: 1, z: 0 });
    expect(result).toEqual({ x: 0, y: 0, z: 1 });
  });

  it("vec3Length", () => {
    expect(vec3Length({ x: 3, y: 4, z: 0 })).toBe(5);
    expect(vec3Length({ x: 0, y: 0, z: 0 })).toBe(0);
  });

  it("vec3Normalize", () => {
    const n = vec3Normalize({ x: 3, y: 0, z: 0 });
    expect(n.x).toBeCloseTo(1);
    expect(n.y).toBeCloseTo(0);
    expect(n.z).toBeCloseTo(0);
  });

  it("vec3Normalize returns fallback for zero vector", () => {
    const n = vec3Normalize({ x: 0, y: 0, z: 0 });
    expect(n).toEqual({ x: 0, y: 0, z: 1 });
  });

  it("vec3Negate", () => {
    expect(vec3Negate({ x: 1, y: -2, z: 3 })).toEqual({
      x: -1,
      y: 2,
      z: -3,
    });
  });

  it("vec3Zero", () => {
    expect(vec3Zero()).toEqual({ x: 0, y: 0, z: 0 });
  });
});
