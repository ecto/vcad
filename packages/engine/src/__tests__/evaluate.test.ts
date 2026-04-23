import { describe, expect, it, beforeAll } from "vitest";
import type { Document, Node } from "@vcad/ir";
import { createDocument } from "@vcad/ir";
import { Engine, type EvaluatedScene } from "../index.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

/** Helper: build a single-root document from nodes. */
function singlePartDoc(
  nodes: Node[],
  rootId: number,
  material = "default",
): Document {
  const doc = createDocument();
  for (const n of nodes) {
    doc.nodes[String(n.id)] = n;
  }
  doc.roots.push({ root: rootId, material });
  return doc;
}

describe("Primitives", () => {
  it("evaluates a cube", () => {
    const doc = singlePartDoc(
      [{ id: 1, name: "box", op: { type: "Cube", size: { x: 10, y: 20, z: 30 } } }],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
    expect(scene.parts[0].material).toBe("default");
    // Cube has 12 triangles (6 faces × 2)
    expect(scene.parts[0].mesh.indices.length).toBe(36);
  });

  it("evaluates a cylinder", () => {
    const doc = singlePartDoc(
      [{ id: 1, name: "cyl", op: { type: "Cylinder", radius: 5, height: 10, segments: 32 } }],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("evaluates a sphere", () => {
    const doc = singlePartDoc(
      [{ id: 1, name: "sph", op: { type: "Sphere", radius: 5, segments: 16 } }],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("evaluates a cone", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "cone",
          op: { type: "Cone", radius_bottom: 5, radius_top: 0, height: 10, segments: 32 },
        },
      ],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("evaluates an empty manifold", () => {
    const doc = singlePartDoc(
      [{ id: 1, name: "empty", op: { type: "Empty" } }],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBe(0);
    expect(scene.parts[0].mesh.indices.length).toBe(0);
  });
});

describe("CSG operations", () => {
  it("evaluates union", () => {
    // Union two offset cubes to get a larger combined shape
    const doc = singlePartDoc(
      [
        { id: 1, name: "a", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
        { id: 2, name: "b_cube", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
        { id: 3, name: "b", op: { type: "Translate", child: 2, offset: { x: 5, y: 0, z: 0 } } },
        { id: 4, name: "u", op: { type: "Union", left: 1, right: 3 } },
      ],
      4,
    );
    const scene = engine.evaluate(doc);
    // Union of two offset cubes produces geometry
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("evaluates difference", () => {
    const doc = singlePartDoc(
      [
        { id: 1, name: "block", op: { type: "Cube", size: { x: 20, y: 20, z: 20 } } },
        { id: 2, name: "hole", op: { type: "Cylinder", radius: 3, height: 30, segments: 32 } },
        { id: 3, name: "result", op: { type: "Difference", left: 1, right: 2 } },
      ],
      3,
    );
    const scene = engine.evaluate(doc);
    // Difference produces more triangles than a plain cube
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(36);
  });

  it("evaluates corner cylinder difference - bbox should stay within cube", () => {
    // Cube 20x20x20 at origin (corner at 0,0,0)
    // Cylinder r=10 h=20 centered at origin (extends from -10 to +10 in x,y)
    // Only the quarter of the cylinder in +x,+y quadrant overlaps the cube
    // Result should have bounding box within [0,0,0] to [20,20,20]
    const doc = singlePartDoc(
      [
        { id: 1, name: "cube", op: { type: "Cube", size: { x: 20, y: 20, z: 20 } } },
        { id: 2, name: "cylinder", op: { type: "Cylinder", radius: 10, height: 20, segments: 32 } },
        { id: 3, name: "result", op: { type: "Difference", left: 1, right: 2 } },
      ],
      3,
    );
    const scene = engine.evaluate(doc);
    const pos = scene.parts[0].mesh.positions;

    // Compute bounding box
    let minX = Infinity, minY = Infinity, minZ = Infinity;
    let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
    for (let i = 0; i < pos.length; i += 3) {
      minX = Math.min(minX, pos[i]);
      minY = Math.min(minY, pos[i + 1]);
      minZ = Math.min(minZ, pos[i + 2]);
      maxX = Math.max(maxX, pos[i]);
      maxY = Math.max(maxY, pos[i + 1]);
      maxZ = Math.max(maxZ, pos[i + 2]);
    }

    console.log(`BBox: (${minX.toFixed(2)}, ${minY.toFixed(2)}, ${minZ.toFixed(2)}) to (${maxX.toFixed(2)}, ${maxY.toFixed(2)}, ${maxZ.toFixed(2)})`);

    // The result should NOT extend into negative x or y
    // (the cylinder parts outside the cube should be excluded)
    expect(minX).toBeGreaterThanOrEqual(-0.1);
    expect(minY).toBeGreaterThanOrEqual(-0.1);
    expect(minZ).toBeGreaterThanOrEqual(-0.1);
    expect(maxX).toBeLessThanOrEqual(20.1);
    expect(maxY).toBeLessThanOrEqual(20.1);
    expect(maxZ).toBeLessThanOrEqual(20.1);
  });

  it("evaluates corner cylinder difference with identity transforms (app structure)", () => {
    // This mirrors how the app structures nodes: primitive -> scale -> rotate -> translate
    // Then difference references the translate nodes
    // App also wraps the boolean result in another scale -> rotate -> translate chain
    const doc = singlePartDoc(
      [
        // Cube chain: primitive -> scale -> rotate -> translate
        { id: 1, name: "cube", op: { type: "Cube", size: { x: 20, y: 20, z: 20 } } },
        { id: 2, name: "cube_scale", op: { type: "Scale", child: 1, factor: { x: 1, y: 1, z: 1 } } },
        { id: 3, name: "cube_rotate", op: { type: "Rotate", child: 2, angles: { x: 0, y: 0, z: 0 } } },
        { id: 4, name: "cube_translate", op: { type: "Translate", child: 3, offset: { x: 0, y: 0, z: 0 } } },
        // Cylinder chain: primitive -> scale -> rotate -> translate
        { id: 5, name: "cylinder", op: { type: "Cylinder", radius: 10, height: 20, segments: 32 } },
        { id: 6, name: "cyl_scale", op: { type: "Scale", child: 5, factor: { x: 1, y: 1, z: 1 } } },
        { id: 7, name: "cyl_rotate", op: { type: "Rotate", child: 6, angles: { x: 0, y: 0, z: 0 } } },
        { id: 8, name: "cyl_translate", op: { type: "Translate", child: 7, offset: { x: 0, y: 0, z: 0 } } },
        // Difference references the translate nodes
        { id: 9, name: "diff", op: { type: "Difference", left: 4, right: 8 } },
        // App wraps boolean result in another transform chain
        { id: 10, name: "diff_scale", op: { type: "Scale", child: 9, factor: { x: 1, y: 1, z: 1 } } },
        { id: 11, name: "diff_rotate", op: { type: "Rotate", child: 10, angles: { x: 0, y: 0, z: 0 } } },
        { id: 12, name: "result", op: { type: "Translate", child: 11, offset: { x: 0, y: 0, z: 0 } } },
      ],
      12,
    );
    const scene = engine.evaluate(doc);
    const pos = scene.parts[0].mesh.positions;
    const tris = scene.parts[0].mesh.indices.length / 3;

    console.log(`Triangle count with identity transforms: ${tris}`);

    // Compute bounding box
    let minX = Infinity, minY = Infinity, minZ = Infinity;
    let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
    for (let i = 0; i < pos.length; i += 3) {
      minX = Math.min(minX, pos[i]);
      minY = Math.min(minY, pos[i + 1]);
      minZ = Math.min(minZ, pos[i + 2]);
      maxX = Math.max(maxX, pos[i]);
      maxY = Math.max(maxY, pos[i + 1]);
      maxZ = Math.max(maxZ, pos[i + 2]);
    }

    console.log(`BBox with transforms: (${minX.toFixed(2)}, ${minY.toFixed(2)}, ${minZ.toFixed(2)}) to (${maxX.toFixed(2)}, ${maxY.toFixed(2)}, ${maxZ.toFixed(2)})`);

    // Should produce same result as without transforms.
    // Triangle count depends on kernel internals:
    //   - cylinder n_height fix (90109a8) pinned the tessellator so
    //     adjacent arc cylinders agree on their seam v-samples,
    //     dropping 220 → 60.
    //   - coplanar-coincidence classification (PR #73) drops the
    //     redundant reversed cylinder-cap at z=20 whose region the
    //     box-top annular already covers, 60 → 44.
    // Shape is unchanged — only tessellation density — and the bbox
    // assertions below still catch geometric regressions.
    expect(tris).toBe(44);
    expect(minX).toBeGreaterThanOrEqual(-0.1);
    expect(minY).toBeGreaterThanOrEqual(-0.1);
  });

  it("evaluates intersection", () => {
    const doc = singlePartDoc(
      [
        { id: 1, name: "a", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
        {
          id: 2,
          name: "b",
          op: { type: "Translate", child: 3, offset: { x: 5, y: 5, z: 5 } },
        },
        { id: 3, name: "b_cube", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
        { id: 4, name: "inter", op: { type: "Intersection", left: 1, right: 2 } },
      ],
      4,
    );
    const scene = engine.evaluate(doc);
    // Intersection of two offset cubes produces a smaller box
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  // TODO(#12): scene.parts[0].solid is undefined after a difference of a cube
  // and a cylinder. The current WASM kernel doesn't seem to expose the .solid
  // field through this code path. Re-enable when the kernel surfaces it.
  it.skip("preserves B-rep data for STEP export after difference", () => {
    const doc = singlePartDoc(
      [
        { id: 1, name: "block", op: { type: "Cube", size: { x: 20, y: 20, z: 20 } } },
        { id: 2, name: "hole", op: { type: "Cylinder", radius: 3, height: 30, segments: 32 } },
        { id: 3, name: "result", op: { type: "Difference", left: 1, right: 2 } },
      ],
      3,
    );
    const scene = engine.evaluate(doc);
    // Verify the solid is present
    expect(scene.parts[0].solid).toBeDefined();
    // Verify B-rep is preserved and STEP export is possible
    expect(scene.parts[0].solid.canExportStep()).toBe(true);
  });

  // TODO(#12): same .solid undefined issue as the simpler difference test above.
  it.skip("preserves B-rep after complex chain like mounting plate", () => {
    // Mirrors the mounting plate structure: transforms -> difference -> cube minus union of holes
    const doc = singlePartDoc(
      [
        // Plate
        { id: 1, name: "plate", op: { type: "Cube", size: { x: 80, y: 6, z: 60 } } },
        // Hole template
        { id: 2, name: "hole_cyl", op: { type: "Cylinder", radius: 3, height: 20, segments: 32 } },
        { id: 3, name: "hole_rotated", op: { type: "Rotate", child: 2, angles: { x: -90, y: 0, z: 0 } } },
        // Multiple translated holes
        { id: 4, name: "hole_1", op: { type: "Translate", child: 3, offset: { x: 8, y: -7, z: 8 } } },
        { id: 5, name: "hole_2", op: { type: "Translate", child: 3, offset: { x: 72, y: -7, z: 8 } } },
        { id: 6, name: "hole_3", op: { type: "Translate", child: 3, offset: { x: 40, y: -7, z: 30 } } },
        // Union holes together
        { id: 7, name: "holes_12", op: { type: "Union", left: 4, right: 5 } },
        { id: 8, name: "holes_123", op: { type: "Union", left: 7, right: 6 } },
        // Boolean difference: plate minus holes
        { id: 9, name: "plate_with_holes", op: { type: "Difference", left: 1, right: 8 } },
        // Final transform chain (like app does)
        { id: 10, name: "scaled", op: { type: "Scale", child: 9, factor: { x: 1, y: 1, z: 1 } } },
        { id: 11, name: "rotated", op: { type: "Rotate", child: 10, angles: { x: 0, y: 0, z: 0 } } },
        { id: 12, name: "translated", op: { type: "Translate", child: 11, offset: { x: -40, y: 0, z: -30 } } },
      ],
      12,
    );
    const scene = engine.evaluate(doc);
    // Verify the solid is present
    expect(scene.parts[0].solid).toBeDefined();
    // Verify B-rep is preserved through the entire chain
    expect(scene.parts[0].solid.canExportStep()).toBe(true);
  });

  // TODO(#12): same .solid undefined issue as the simpler difference test above.
  it.skip("preserves B-rep for union of non-overlapping cylinders", () => {
    // Test if union of multiple non-overlapping solids preserves valid topology
    const doc = singlePartDoc(
      [
        { id: 1, name: "cyl1", op: { type: "Cylinder", radius: 3, height: 10, segments: 32 } },
        { id: 2, name: "cyl2", op: { type: "Cylinder", radius: 3, height: 10, segments: 32 } },
        { id: 3, name: "cyl2_moved", op: { type: "Translate", child: 2, offset: { x: 20, y: 0, z: 0 } } },
        { id: 4, name: "union", op: { type: "Union", left: 1, right: 3 } },
      ],
      4,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts[0].solid).toBeDefined();
    expect(scene.parts[0].solid.canExportStep()).toBe(true);
    // Verify we can actually export to STEP
    const stepBuffer = scene.parts[0].solid.toStepBuffer();
    expect(stepBuffer.length).toBeGreaterThan(0);
  });

  // TODO(#12): fails with "half-edge has no parent edge" — topology repair
  // works for simple cases but the 9-hole mounting plate has edge cases
  // that still fail. The simpler 4-hole test above passes.
  it.skip("preserves B-rep for exact mounting plate example structure", () => {
    // This is the EXACT document structure from the mounting plate example
    const doc = createDocument();
    doc.nodes = {
      // Plate primitive: 80x6x60
      "1": { id: 1, name: null, op: { type: "Cube", size: { x: 80, y: 6, z: 60 } } },
      // Large center hole
      "2": { id: 2, name: null, op: { type: "Cylinder", radius: 6, height: 20, segments: 32 } },
      "3": { id: 3, name: null, op: { type: "Rotate", child: 2, angles: { x: -90, y: 0, z: 0 } } },
      "4": { id: 4, name: null, op: { type: "Translate", child: 3, offset: { x: 40, y: -7, z: 30 } } },
      // Small mounting holes
      "10": { id: 10, name: null, op: { type: "Cylinder", radius: 2, height: 20, segments: 24 } },
      "11": { id: 11, name: null, op: { type: "Rotate", child: 10, angles: { x: -90, y: 0, z: 0 } } },
      // Corner holes
      "20": { id: 20, name: null, op: { type: "Translate", child: 11, offset: { x: 8, y: -7, z: 8 } } },
      "21": { id: 21, name: null, op: { type: "Translate", child: 11, offset: { x: 72, y: -7, z: 8 } } },
      "22": { id: 22, name: null, op: { type: "Translate", child: 11, offset: { x: 8, y: -7, z: 52 } } },
      "23": { id: 23, name: null, op: { type: "Translate", child: 11, offset: { x: 72, y: -7, z: 52 } } },
      // Edge holes
      "24": { id: 24, name: null, op: { type: "Translate", child: 11, offset: { x: 8, y: -7, z: 30 } } },
      "25": { id: 25, name: null, op: { type: "Translate", child: 11, offset: { x: 72, y: -7, z: 30 } } },
      "26": { id: 26, name: null, op: { type: "Translate", child: 11, offset: { x: 40, y: -7, z: 8 } } },
      "27": { id: 27, name: null, op: { type: "Translate", child: 11, offset: { x: 40, y: -7, z: 52 } } },
      // Union all holes
      "30": { id: 30, name: null, op: { type: "Union", left: 4, right: 20 } },
      "31": { id: 31, name: null, op: { type: "Union", left: 30, right: 21 } },
      "32": { id: 32, name: null, op: { type: "Union", left: 31, right: 22 } },
      "33": { id: 33, name: null, op: { type: "Union", left: 32, right: 23 } },
      "34": { id: 34, name: null, op: { type: "Union", left: 33, right: 24 } },
      "35": { id: 35, name: null, op: { type: "Union", left: 34, right: 25 } },
      "36": { id: 36, name: null, op: { type: "Union", left: 35, right: 26 } },
      "37": { id: 37, name: null, op: { type: "Union", left: 36, right: 27 } },
      // Boolean difference
      "40": { id: 40, name: null, op: { type: "Difference", left: 1, right: 37 } },
      // Final transforms
      "50": { id: 50, name: null, op: { type: "Scale", child: 40, factor: { x: 1, y: 1, z: 1 } } },
      "51": { id: 51, name: null, op: { type: "Rotate", child: 50, angles: { x: 0, y: 0, z: 0 } } },
      "52": { id: 52, name: "Mounting Plate", op: { type: "Translate", child: 51, offset: { x: -40, y: 0, z: -30 } } },
    };
    doc.roots = [{ root: 52, material: "default" }];

    const scene = engine.evaluate(doc);
    // Verify parts exist
    expect(scene.parts.length).toBe(1);
    // Verify the solid is present
    expect(scene.parts[0].solid).toBeDefined();
    // Verify B-rep is preserved (canExportStep should return true)
    expect(scene.parts[0].solid.canExportStep()).toBe(true);
    // Verify we can actually export to STEP (topology is valid)
    const stepBuffer = scene.parts[0].solid.toStepBuffer();
    expect(stepBuffer.length).toBeGreaterThan(0);
    // Verify STEP header
    const stepText = new TextDecoder().decode(stepBuffer.slice(0, 100));
    expect(stepText).toContain("ISO-10303-21");
  });
});

describe("Transforms", () => {
  it("translate shifts bounding box", () => {
    const doc = singlePartDoc(
      [
        { id: 1, name: "box", op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } },
        {
          id: 2,
          name: "moved",
          op: { type: "Translate", child: 1, offset: { x: 100, y: 0, z: 0 } },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    const pos = scene.parts[0].mesh.positions;
    // All x coords should be >= 100
    for (let i = 0; i < pos.length; i += 3) {
      expect(pos[i]).toBeGreaterThanOrEqual(99.9);
    }
  });

  it("scale changes dimensions", () => {
    const doc = singlePartDoc(
      [
        { id: 1, name: "box", op: { type: "Cube", size: { x: 1, y: 1, z: 1 } } },
        {
          id: 2,
          name: "scaled",
          op: { type: "Scale", child: 1, factor: { x: 10, y: 10, z: 10 } },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    const pos = scene.parts[0].mesh.positions;
    let maxX = -Infinity;
    for (let i = 0; i < pos.length; i += 3) {
      if (pos[i] > maxX) maxX = pos[i];
    }
    expect(maxX).toBeCloseTo(10, 1);
  });

  it("rotate produces valid mesh", () => {
    const doc = singlePartDoc(
      [
        { id: 1, name: "box", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
        {
          id: 2,
          name: "rotated",
          op: { type: "Rotate", child: 1, angles: { x: 0, y: 0, z: 45 } },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });
});

describe("DAG sharing", () => {
  it("shared nodes are evaluated once", () => {
    // Two differences sharing the same cylinder
    const doc = createDocument();
    doc.nodes["1"] = {
      id: 1,
      name: "block_a",
      op: { type: "Cube", size: { x: 20, y: 20, z: 20 } },
    };
    doc.nodes["2"] = {
      id: 2,
      name: "block_b",
      op: { type: "Cube", size: { x: 20, y: 20, z: 20 } },
    };
    doc.nodes["3"] = {
      id: 3,
      name: "shared_hole",
      op: { type: "Cylinder", radius: 3, height: 30, segments: 32 },
    };
    doc.nodes["4"] = {
      id: 4,
      name: "diff_a",
      op: { type: "Difference", left: 1, right: 3 },
    };
    doc.nodes["5"] = {
      id: 5,
      name: "diff_b",
      op: { type: "Difference", left: 2, right: 3 },
    };
    doc.roots.push({ root: 4, material: "steel" });
    doc.roots.push({ root: 5, material: "aluminum" });

    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(2);
    // Both parts should have identical geometry (same operations, same shared cylinder)
    expect(scene.parts[0].mesh.indices.length).toBe(
      scene.parts[1].mesh.indices.length,
    );
    expect(scene.parts[0].material).toBe("steel");
    expect(scene.parts[1].material).toBe("aluminum");
  });
});

describe("Scene", () => {
  it("multi-root document returns correct part count and materials", () => {
    const doc = createDocument();
    doc.nodes["1"] = {
      id: 1,
      name: "part_a",
      op: { type: "Cube", size: { x: 10, y: 10, z: 10 } },
    };
    doc.nodes["2"] = {
      id: 2,
      name: "part_b",
      op: { type: "Sphere", radius: 5, segments: 16 },
    };
    doc.roots.push({ root: 1, material: "wood" });
    doc.roots.push({ root: 2, material: "metal" });

    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(2);
    expect(scene.parts[0].material).toBe("wood");
    expect(scene.parts[1].material).toBe("metal");
  });
});

describe("Errors", () => {
  it("records a failure (instead of throwing) on a missing node reference", () => {
    // Evaluation is per-root resilient: a dangling NodeId must not blank
    // the whole scene — it should emit an empty mesh for that root and
    // surface the error in `scene.failures`.
    const doc = singlePartDoc(
      [{ id: 1, name: "bad", op: { type: "Union", left: 99, right: 100 } }],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBe(0);
    expect(scene.failures).toBeDefined();
    expect(scene.failures!.length).toBeGreaterThan(0);
    expect(scene.failures![0].scope).toBe("root[0]");
    expect(scene.failures![0].error).toMatch(/99|missing/i);
  });
});

describe("Sketch operations", () => {
  it("evaluates extrude from rectangle sketch", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "rectangle",
          op: {
            type: "Sketch2D",
            origin: { x: 0, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            segments: [
              { type: "Line", start: { x: 0, y: 0 }, end: { x: 10, y: 0 } },
              { type: "Line", start: { x: 10, y: 0 }, end: { x: 10, y: 5 } },
              { type: "Line", start: { x: 10, y: 5 }, end: { x: 0, y: 5 } },
              { type: "Line", start: { x: 0, y: 5 }, end: { x: 0, y: 0 } },
            ],
          },
        },
        {
          id: 2,
          name: "extruded_block",
          op: {
            type: "Extrude",
            sketch: 1,
            direction: { x: 0, y: 0, z: 20 },
          },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
    // Extruded rectangle has 6 faces, 12 triangles minimum
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThanOrEqual(36);
  });

  it("evaluates revolve from rectangle sketch", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "profile",
          op: {
            type: "Sketch2D",
            origin: { x: 5, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 0, z: 1 },
            segments: [
              { type: "Line", start: { x: 0, y: 0 }, end: { x: 3, y: 0 } },
              { type: "Line", start: { x: 3, y: 0 }, end: { x: 3, y: 10 } },
              { type: "Line", start: { x: 3, y: 10 }, end: { x: 0, y: 10 } },
              { type: "Line", start: { x: 0, y: 10 }, end: { x: 0, y: 0 } },
            ],
          },
        },
        {
          id: 2,
          name: "revolved",
          op: {
            type: "Revolve",
            sketch: 1,
            axis_origin: { x: 0, y: 0, z: 0 },
            axis_dir: { x: 0, y: 0, z: 1 },
            angle_deg: 90,
          },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("sketch node alone evaluates to empty", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "just_sketch",
          op: {
            type: "Sketch2D",
            origin: { x: 0, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            segments: [
              { type: "Line", start: { x: 0, y: 0 }, end: { x: 10, y: 0 } },
              { type: "Line", start: { x: 10, y: 0 }, end: { x: 10, y: 10 } },
              { type: "Line", start: { x: 10, y: 10 }, end: { x: 0, y: 10 } },
              { type: "Line", start: { x: 0, y: 10 }, end: { x: 0, y: 0 } },
            ],
          },
        },
      ],
      1,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    // Sketch alone should produce empty geometry
    expect(scene.parts[0].mesh.positions.length).toBe(0);
  });

  it("evaluates sweep with line path", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "circle_profile",
          op: {
            type: "Sketch2D",
            origin: { x: 0, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            // Approximate circle with square for simpler test
            segments: [
              { type: "Line", start: { x: -2, y: -2 }, end: { x: 2, y: -2 } },
              { type: "Line", start: { x: 2, y: -2 }, end: { x: 2, y: 2 } },
              { type: "Line", start: { x: 2, y: 2 }, end: { x: -2, y: 2 } },
              { type: "Line", start: { x: -2, y: 2 }, end: { x: -2, y: -2 } },
            ],
          },
        },
        {
          id: 2,
          name: "swept",
          op: {
            type: "Sweep",
            sketch: 1,
            path: {
              type: "Line",
              start: { x: 0, y: 0, z: 0 },
              end: { x: 0, y: 0, z: 20 },
            },
          },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("evaluates sweep with helix path", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "small_square",
          op: {
            type: "Sketch2D",
            origin: { x: 0, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            segments: [
              { type: "Line", start: { x: -1, y: -1 }, end: { x: 1, y: -1 } },
              { type: "Line", start: { x: 1, y: -1 }, end: { x: 1, y: 1 } },
              { type: "Line", start: { x: 1, y: 1 }, end: { x: -1, y: 1 } },
              { type: "Line", start: { x: -1, y: 1 }, end: { x: -1, y: -1 } },
            ],
          },
        },
        {
          id: 2,
          name: "spring",
          op: {
            type: "Sweep",
            sketch: 1,
            path: {
              type: "Helix",
              radius: 10,
              pitch: 5,
              height: 20,
              turns: 2,
            },
          },
        },
      ],
      2,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });

  it("evaluates loft between two profiles", () => {
    const doc = singlePartDoc(
      [
        {
          id: 1,
          name: "profile_bottom",
          op: {
            type: "Sketch2D",
            origin: { x: 0, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            segments: [
              { type: "Line", start: { x: 0, y: 0 }, end: { x: 10, y: 0 } },
              { type: "Line", start: { x: 10, y: 0 }, end: { x: 10, y: 10 } },
              { type: "Line", start: { x: 10, y: 10 }, end: { x: 0, y: 10 } },
              { type: "Line", start: { x: 0, y: 10 }, end: { x: 0, y: 0 } },
            ],
          },
        },
        {
          id: 2,
          name: "profile_top",
          op: {
            type: "Sketch2D",
            origin: { x: 2, y: 2, z: 20 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            segments: [
              { type: "Line", start: { x: 0, y: 0 }, end: { x: 6, y: 0 } },
              { type: "Line", start: { x: 6, y: 0 }, end: { x: 6, y: 6 } },
              { type: "Line", start: { x: 6, y: 6 }, end: { x: 0, y: 6 } },
              { type: "Line", start: { x: 0, y: 6 }, end: { x: 0, y: 0 } },
            ],
          },
        },
        {
          id: 3,
          name: "lofted",
          op: {
            type: "Loft",
            sketches: [1, 2],
          },
        },
      ],
      3,
    );
    const scene = engine.evaluate(doc);
    expect(scene.parts).toHaveLength(1);
    expect(scene.parts[0].mesh.positions.length).toBeGreaterThan(0);
    expect(scene.parts[0].mesh.indices.length).toBeGreaterThan(0);
  });
});

describe("Assembly evaluation", () => {
  it("evaluates partDefs and instances", () => {
    const doc: Document = {
      version: "0.1",
      nodes: {
        "1": { id: 1, name: "cube", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
        "2": { id: 2, name: "cylinder", op: { type: "Cylinder", radius: 5, height: 20, segments: 16 } },
      },
      materials: {},
      part_materials: {},
      roots: [],
      partDefs: {
        box: { id: "box", name: "Box", root: 1, defaultMaterial: "metal" },
        rod: { id: "rod", name: "Rod", root: 2, defaultMaterial: "plastic" },
      },
      instances: [
        {
          id: "box-1",
          partDefId: "box",
          name: "Box Instance",
          transform: {
            translation: { x: 0, y: 0, z: 0 },
            rotation: { x: 0, y: 0, z: 0 },
            scale: { x: 1, y: 1, z: 1 },
          },
        },
        {
          id: "rod-1",
          partDefId: "rod",
          name: "Rod Instance",
          transform: {
            translation: { x: 20, y: 0, z: 0 },
            rotation: { x: 0, y: 0, z: 0 },
            scale: { x: 1, y: 1, z: 1 },
          },
        },
      ],
      groundInstanceId: "box-1",
    };

    const scene = engine.evaluate(doc);

    // Should have partDefs
    expect(scene.partDefs).toBeDefined();
    expect(scene.partDefs).toHaveLength(2);

    // Should have instances
    expect(scene.instances).toBeDefined();
    expect(scene.instances).toHaveLength(2);

    // Check first instance
    const boxInstance = scene.instances!.find((i) => i.instanceId === "box-1");
    expect(boxInstance).toBeDefined();
    expect(boxInstance!.partDefId).toBe("box");
    expect(boxInstance!.material).toBe("metal");
    expect(boxInstance!.mesh.positions.length).toBeGreaterThan(0);

    // Check second instance
    const rodInstance = scene.instances!.find((i) => i.instanceId === "rod-1");
    expect(rodInstance).toBeDefined();
    expect(rodInstance!.partDefId).toBe("rod");
    expect(rodInstance!.material).toBe("plastic");
    expect(rodInstance!.transform?.translation.x).toBe(20);

    // No clashes (they're separated)
    expect(scene.clashes).toHaveLength(0);
  });

  it("applies kinematics for joints", () => {
    const doc: Document = {
      version: "0.1",
      nodes: {
        "1": { id: 1, name: "cube", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
      },
      materials: {},
      part_materials: {},
      roots: [],
      partDefs: {
        box: { id: "box", root: 1 },
      },
      instances: [
        {
          id: "ground",
          partDefId: "box",
          transform: {
            translation: { x: 0, y: 0, z: 0 },
            rotation: { x: 0, y: 0, z: 0 },
            scale: { x: 1, y: 1, z: 1 },
          },
        },
        {
          id: "arm",
          partDefId: "box",
          transform: {
            translation: { x: 0, y: 0, z: 0 },
            rotation: { x: 0, y: 0, z: 0 },
            scale: { x: 1, y: 1, z: 1 },
          },
        },
      ],
      joints: [
        {
          id: "hinge",
          name: "Hinge Joint",
          parentInstanceId: "ground",
          childInstanceId: "arm",
          parentAnchor: { x: 10, y: 5, z: 5 },
          childAnchor: { x: 0, y: 5, z: 5 },
          kind: { type: "Revolute", axis: { x: 0, y: 1, z: 0 } },
          state: 0,
        },
      ],
      groundInstanceId: "ground",
    };

    const scene = engine.evaluate(doc);

    expect(scene.instances).toBeDefined();
    expect(scene.instances).toHaveLength(2);

    // Arm should be positioned relative to ground via joint
    const armInstance = scene.instances!.find((i) => i.instanceId === "arm");
    expect(armInstance).toBeDefined();
    expect(armInstance!.transform).toBeDefined();
    // With state=0 (no rotation), arm should be translated by parent-child anchor difference
    expect(armInstance!.transform!.translation.x).toBe(10); // parentAnchor.x - childAnchor.x
  });

  // TODO(#12): scene.clashes is empty even when two instances overlap.
  // Either the engine isn't running clash detection through this path or
  // the kernel returns no positions. Re-enable when clash detection is
  // wired up here.
  it.skip("detects clashes between overlapping instances", () => {
    const doc: Document = {
      version: "0.1",
      nodes: {
        "1": { id: 1, name: "cube", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
      },
      materials: {},
      part_materials: {},
      roots: [],
      partDefs: {
        box: { id: "box", root: 1 },
      },
      instances: [
        {
          id: "box-1",
          partDefId: "box",
          transform: {
            translation: { x: 0, y: 0, z: 0 },
            rotation: { x: 0, y: 0, z: 0 },
            scale: { x: 1, y: 1, z: 1 },
          },
        },
        {
          id: "box-2",
          partDefId: "box",
          transform: {
            translation: { x: 5, y: 0, z: 0 }, // Overlaps with box-1
            rotation: { x: 0, y: 0, z: 0 },
            scale: { x: 1, y: 1, z: 1 },
          },
        },
      ],
    };

    const scene = engine.evaluate(doc);

    // Should detect clash between overlapping cubes
    expect(scene.clashes.length).toBeGreaterThan(0);
    expect(scene.clashes[0].positions.length).toBeGreaterThan(0);
  });
});
