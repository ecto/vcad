import { describe, expect, it } from "vitest";
import { fromVCode, toVCode, VCodeParseError, createDocument } from "../index.js";
import type { Node, CsgOp } from "../index.js";

describe("VCode", () => {
  describe("fromVCode", () => {
    it("parses a simple cube", () => {
      const doc = fromVCode("C 50 30 5");
      expect(Object.keys(doc.nodes)).toHaveLength(1);
      const node = doc.nodes["0"];
      expect(node.op.type).toBe("Cube");
      if (node.op.type === "Cube") {
        expect(node.op.size).toEqual({ x: 50, y: 30, z: 5 });
      }
    });

    it("parses plate with hole example", () => {
      const compact = "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2";
      const doc = fromVCode(compact);
      expect(Object.keys(doc.nodes)).toHaveLength(4);

      // Node 0: Cube
      expect(doc.nodes["0"].op.type).toBe("Cube");
      if (doc.nodes["0"].op.type === "Cube") {
        expect(doc.nodes["0"].op.size).toEqual({ x: 50, y: 30, z: 5 });
      }

      // Node 1: Cylinder
      expect(doc.nodes["1"].op.type).toBe("Cylinder");
      if (doc.nodes["1"].op.type === "Cylinder") {
        expect(doc.nodes["1"].op.radius).toBe(5);
        expect(doc.nodes["1"].op.height).toBe(10);
      }

      // Node 2: Translate
      expect(doc.nodes["2"].op.type).toBe("Translate");
      if (doc.nodes["2"].op.type === "Translate") {
        expect(doc.nodes["2"].op.child).toBe(1);
        expect(doc.nodes["2"].op.offset).toEqual({ x: 25, y: 15, z: 0 });
      }

      // Node 3: Difference
      expect(doc.nodes["3"].op.type).toBe("Difference");
      if (doc.nodes["3"].op.type === "Difference") {
        expect(doc.nodes["3"].op.left).toBe(0);
        expect(doc.nodes["3"].op.right).toBe(2);
      }

      // Root should be node 3
      expect(doc.roots[0].root).toBe(3);
    });

    it("parses all primitives", () => {
      const compact = "C 10 20 30\nY 5 15\nS 8\nK 5 2 20";
      const doc = fromVCode(compact);
      expect(Object.keys(doc.nodes)).toHaveLength(4);

      expect(doc.nodes["0"].op.type).toBe("Cube");
      expect(doc.nodes["1"].op.type).toBe("Cylinder");
      expect(doc.nodes["2"].op.type).toBe("Sphere");
      expect(doc.nodes["3"].op.type).toBe("Cone");

      if (doc.nodes["3"].op.type === "Cone") {
        expect(doc.nodes["3"].op.radius_bottom).toBe(5);
        expect(doc.nodes["3"].op.radius_top).toBe(2);
        expect(doc.nodes["3"].op.height).toBe(20);
      }
    });

    it("parses all booleans", () => {
      const compact = "C 10 10 10\nC 5 5 5\nU 0 1\nD 0 1\nI 0 1";
      const doc = fromVCode(compact);

      expect(doc.nodes["2"].op.type).toBe("Union");
      expect(doc.nodes["3"].op.type).toBe("Difference");
      expect(doc.nodes["4"].op.type).toBe("Intersection");
    });

    it("parses all transforms", () => {
      const compact = "C 10 10 10\nT 0 5 10 15\nR 1 45 0 90\nX 2 2 2 2";
      const doc = fromVCode(compact);

      if (doc.nodes["1"].op.type === "Translate") {
        expect(doc.nodes["1"].op.offset).toEqual({ x: 5, y: 10, z: 15 });
      }
      if (doc.nodes["2"].op.type === "Rotate") {
        expect(doc.nodes["2"].op.angles).toEqual({ x: 45, y: 0, z: 90 });
      }
      if (doc.nodes["3"].op.type === "Scale") {
        expect(doc.nodes["3"].op.factor).toEqual({ x: 2, y: 2, z: 2 });
      }
    });

    it("parses linear pattern", () => {
      const compact = "C 10 10 5\nLP 0 1 0 0 5 20";
      const doc = fromVCode(compact);

      expect(doc.nodes["1"].op.type).toBe("LinearPattern");
      if (doc.nodes["1"].op.type === "LinearPattern") {
        expect(doc.nodes["1"].op.child).toBe(0);
        expect(doc.nodes["1"].op.direction).toEqual({ x: 1, y: 0, z: 0 });
        expect(doc.nodes["1"].op.count).toBe(5);
        expect(doc.nodes["1"].op.spacing).toBe(20);
      }
    });

    it("parses circular pattern", () => {
      const compact = "Y 3 10\nCP 0 0 0 0 0 0 1 6 360";
      const doc = fromVCode(compact);

      expect(doc.nodes["1"].op.type).toBe("CircularPattern");
      if (doc.nodes["1"].op.type === "CircularPattern") {
        expect(doc.nodes["1"].op.count).toBe(6);
        expect(doc.nodes["1"].op.angle_deg).toBe(360);
      }
    });

    it("parses shell", () => {
      const compact = "C 50 50 50\nSH 0 2";
      const doc = fromVCode(compact);

      expect(doc.nodes["1"].op.type).toBe("Shell");
      if (doc.nodes["1"].op.type === "Shell") {
        expect(doc.nodes["1"].op.child).toBe(0);
        expect(doc.nodes["1"].op.thickness).toBe(2);
      }
    });

    it("parses sketch and extrude", () => {
      const compact = "SK 0 0 0  1 0 0  0 1 0\nL 0 0 10 0\nL 10 0 10 5\nL 10 5 0 5\nL 0 5 0 0\nEND\nE 0 0 0 20";
      const doc = fromVCode(compact);

      expect(doc.nodes["0"].op.type).toBe("Sketch2D");
      if (doc.nodes["0"].op.type === "Sketch2D") {
        expect(doc.nodes["0"].op.segments).toHaveLength(4);
      }

      // Extrude is at the line after END
      const extrudeKey = Object.keys(doc.nodes).find(k => doc.nodes[k].op.type === "Extrude");
      expect(extrudeKey).toBeDefined();
      if (extrudeKey && doc.nodes[extrudeKey].op.type === "Extrude") {
        expect(doc.nodes[extrudeKey].op.sketch).toBe(0);
        expect(doc.nodes[extrudeKey].op.direction).toEqual({ x: 0, y: 0, z: 20 });
      }
    });

    it("parses sketch with arc", () => {
      const compact = "SK 0 0 0  1 0 0  0 1 0\nL 0 0 10 0\nA 10 0 10 10 10 5 1\nL 10 10 0 10\nL 0 10 0 0\nEND";
      const doc = fromVCode(compact);

      if (doc.nodes["0"].op.type === "Sketch2D") {
        expect(doc.nodes["0"].op.segments).toHaveLength(4);
        const arc = doc.nodes["0"].op.segments[1];
        expect(arc.type).toBe("Arc");
        if (arc.type === "Arc") {
          expect(arc.ccw).toBe(true);
        }
      }
    });

    it("skips comments and empty lines", () => {
      const compact = "# Comment\nC 10 10 10\n\n# Another comment\nY 5 10";
      const doc = fromVCode(compact);
      // Nodes are created at line numbers 1 and 4
      expect(Object.keys(doc.nodes)).toHaveLength(2);
    });

    it("handles negative numbers", () => {
      const compact = "C 10 10 10\nT 0 -5 -10 -15";
      const doc = fromVCode(compact);

      if (doc.nodes["1"].op.type === "Translate") {
        expect(doc.nodes["1"].op.offset).toEqual({ x: -5, y: -10, z: -15 });
      }
    });

    it("handles floating point numbers", () => {
      const compact = "C 10.5 20.25 30.125";
      const doc = fromVCode(compact);

      if (doc.nodes["0"].op.type === "Cube") {
        expect(doc.nodes["0"].op.size.x).toBe(10.5);
        expect(doc.nodes["0"].op.size.y).toBe(20.25);
        expect(doc.nodes["0"].op.size.z).toBe(30.125);
      }
    });

    it("handles empty input", () => {
      const doc = fromVCode("");
      expect(Object.keys(doc.nodes)).toHaveLength(0);
      expect(doc.roots).toHaveLength(0);
    });

    it("throws on invalid opcode", () => {
      expect(() => fromVCode("Z 10 10 10")).toThrow(VCodeParseError);
    });

    it("throws on wrong arg count", () => {
      expect(() => fromVCode("C 10 10")).toThrow(VCodeParseError);
    });
  });

  describe("toVCode", () => {
    it("serializes a simple cube", () => {
      const doc = createDocument();
      doc.nodes["0"] = {
        id: 0,
        name: null,
        op: { type: "Cube", size: { x: 50, y: 30, z: 5 } },
      };
      doc.roots.push({ root: 0, material: "default" });

      const compact = toVCode(doc);
      // v0.2 format includes header, geometry section, and scene roots
      expect(compact).toContain("# vcad 0.2");
      expect(compact).toContain("C 50 30 5");
      expect(compact).toContain("ROOT 0 default");
    });

    it("roundtrips plate with hole", () => {
      const original = "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2";
      const doc = fromVCode(original);
      const compact = toVCode(doc);

      // Parse again and verify structure
      const doc2 = fromVCode(compact);
      expect(Object.keys(doc2.nodes)).toHaveLength(4);
      expect(doc2.roots[0].root).toBe(3);
    });

    it("serializes all primitives", () => {
      const doc = createDocument();
      doc.nodes["0"] = { id: 0, name: null, op: { type: "Cube", size: { x: 10, y: 20, z: 30 } } };
      doc.nodes["1"] = { id: 1, name: null, op: { type: "Cylinder", radius: 5, height: 15, segments: 32 } };
      doc.nodes["2"] = { id: 2, name: null, op: { type: "Sphere", radius: 8, segments: 32 } };
      doc.nodes["3"] = { id: 3, name: null, op: { type: "Cone", radius_bottom: 5, radius_top: 2, height: 20, segments: 32 } };
      doc.roots.push({ root: 3, material: "default" });

      const compact = toVCode(doc);
      // v0.2 format includes header and sections, check for content
      expect(compact).toContain("C 10 20 30");
      expect(compact).toContain("Y 5 15");
      expect(compact).toContain("S 8");
      expect(compact).toContain("K 5 2 20");
    });

    it("handles empty document", () => {
      const doc = createDocument();
      const compact = toVCode(doc);
      // Empty document still has version header
      expect(compact).toBe("# vcad 0.2");
    });
  });

  describe("joints", () => {
    it("round-trips a Free (6-DOF) joint through JFRE", () => {
      const doc = fromVCode("JFRE base _ chassis 1 2 3 4 5 6");
      expect(doc.joints).toHaveLength(1);
      const joint = doc.joints[0];
      expect(joint.id).toBe("base");
      expect(joint.parentInstanceId).toBeNull();
      expect(joint.childInstanceId).toBe("chassis");
      expect(joint.parentAnchor).toEqual({ x: 1, y: 2, z: 3 });
      expect(joint.childAnchor).toEqual({ x: 4, y: 5, z: 6 });
      expect(joint.kind.type).toBe("Free");

      expect(toVCode(doc)).toContain("JFRE base _ chassis 1 2 3 4 5 6");
    });

    it("rejects a JFRE line with too few args", () => {
      expect(() => fromVCode("JFRE base _ chassis 1 2 3")).toThrow(VCodeParseError);
    });
  });

  describe("complex models", () => {
    it("parses flange with bolt holes", () => {
      const compact = `Y 25 5
Y 3 10
T 1 15 0 0
CP 2 0 0 0 0 0 1 6 360
D 0 3`;

      const doc = fromVCode(compact);
      expect(Object.keys(doc.nodes)).toHaveLength(5);
      expect(doc.roots[0].root).toBe(4);
    });
  });
});
