/**
 * Tests for part generators.
 */

import { describe, it, expect } from "vitest";
import { fromVCode } from "@vcad/ir";
import {
  PlateGenerator,
  SpacerGenerator,
  BracketGenerator,
  FlangeGenerator,
  ShaftGenerator,
  EnclosureGenerator,
  MountGenerator,
  // New generators
  BallGenerator,
  FunnelGenerator,
  ClipGenerator,
  ScaledGenerator,
  ArrayGenerator,
  RadialGenerator,
  HollowGenerator,
  ProfileGenerator,
  TurnedGenerator,
  generators,
  generatorFamilies,
  generateRandomPart,
} from "../generators/index.js";
import { validateExample } from "../validate.js";
import { generateSyntheticDescription } from "../annotate.js";
import { generateConversation, generateConversations, toShareGPTFormat } from "../conversation.js";

describe("PlateGenerator", () => {
  const gen = new PlateGenerator();

  it("generates valid plate without holes", () => {
    const part = gen.generate({ holePattern: "none" });
    expect(part.family).toBe("plate");
    expect(part.vcode).toMatch(/^C \d/); // Starts with cube

    // Parse should succeed
    const doc = fromVCode(part.vcode);
    expect(Object.keys(doc.nodes).length).toBeGreaterThan(0);
  });

  it("generates valid plate with corner holes", () => {
    const part = gen.generate({ holePattern: "corners" });
    expect(part.vcode).toContain("D"); // Should have difference ops

    const doc = fromVCode(part.vcode);
    expect(Object.keys(doc.nodes).length).toBeGreaterThan(4); // Base + 4 holes
  });

  it("generates valid plate with grid holes", () => {
    const part = gen.generate({ holePattern: "grid", gridCols: 3, gridRows: 2 });
    expect(part.vcode).toContain("D");

    const doc = fromVCode(part.vcode);
    expect(doc.roots.length).toBe(1);
  });

  it("has correct param definitions", () => {
    const defs = gen.paramDefs();
    expect(defs.width.type).toBe("number");
    expect(defs.holePattern.type).toBe("choice");
    expect(defs.holePattern.choices).toContain("corners");
  });
});

describe("SpacerGenerator", () => {
  const gen = new SpacerGenerator();

  it("generates solid spacer", () => {
    const part = gen.generate({ spacerType: "solid" });
    expect(part.vcode).toMatch(/^Y \d/); // Just a cylinder
    expect(part.complexity).toBe(1);
  });

  it("generates hollow spacer", () => {
    const part = gen.generate({ spacerType: "hollow" });
    expect(part.vcode).toContain("D"); // Difference for hole
  });

  it("generates flanged spacer", () => {
    const part = gen.generate({ spacerType: "flanged" });
    expect(part.vcode).toContain("U"); // Union for flange
    expect(part.vcode).toContain("D"); // Difference for center hole
  });
});

describe("BracketGenerator", () => {
  const gen = new BracketGenerator();

  it("generates simple L-bracket", () => {
    const part = gen.generate({ bracketType: "simple", hasHoles: false });
    expect(part.vcode).toContain("U"); // Union of two legs
    expect(part.family).toBe("bracket");
  });

  it("generates gusseted bracket", () => {
    const part = gen.generate({ bracketType: "gusseted", hasHoles: false });
    expect(part.vcode.match(/U/g)?.length).toBeGreaterThanOrEqual(2); // Extra union for gusset
  });

  it("generates bracket with holes", () => {
    const part = gen.generate({ bracketType: "simple", hasHoles: true });
    expect(part.vcode).toContain("D"); // Differences for holes
  });
});

describe("FlangeGenerator", () => {
  const gen = new FlangeGenerator();

  it("generates flat flange", () => {
    const part = gen.generate({ flangeType: "flat" });
    expect(part.vcode).toMatch(/^Y \d/); // Base cylinder
    expect(part.vcode).toContain("D"); // Center hole and bolt holes
  });

  it("generates hubbed flange", () => {
    const part = gen.generate({ flangeType: "hubbed" });
    expect(part.vcode).toContain("U"); // Hub union
  });

  it("has correct bolt pattern", () => {
    const part = gen.generate({ boltCount: 6 });
    // Count cylinder operations (should have base + center hole + 6 bolt holes)
    const cylCount = (part.vcode.match(/^Y /gm) || []).length;
    expect(cylCount).toBeGreaterThanOrEqual(7);
  });
});

describe("ShaftGenerator", () => {
  const gen = new ShaftGenerator();

  it("generates simple shaft", () => {
    const part = gen.generate({ shaftType: "simple", hasCenterHole: false, hasKeyway: false });
    expect(part.vcode).toMatch(/^Y \d/);
    expect(part.complexity).toBe(1);
  });

  it("generates stepped shaft", () => {
    const part = gen.generate({ shaftType: "stepped2", hasCenterHole: false, hasKeyway: false });
    expect(part.vcode).toContain("U"); // Union of sections
  });

  it("generates shaft with keyway", () => {
    const part = gen.generate({ shaftType: "stepped2", hasKeyway: true, hasCenterHole: false });
    expect(part.vcode).toContain("D"); // Difference for keyway
    expect(part.vcode).toContain("C "); // Cube for keyway slot
  });
});

describe("EnclosureGenerator", () => {
  const gen = new EnclosureGenerator();

  it("generates box enclosure", () => {
    const part = gen.generate({ enclosureType: "box", hasFlange: false });
    expect(part.vcode).toMatch(/^C \d/); // Outer cube
    expect(part.vcode).toContain("D"); // Hollow out
  });

  it("generates lid", () => {
    const part = gen.generate({ enclosureType: "lid" });
    expect(part.vcode).toContain("U"); // Lip union
  });

  it("generates box with standoffs", () => {
    const part = gen.generate({ enclosureType: "boxWithStandoffs", hasFlange: false });
    expect(part.vcode).toContain("Y"); // Standoff cylinders
    expect(part.complexity).toBe(4);
  });
});

describe("MountGenerator", () => {
  const gen = new MountGenerator();

  it("generates NEMA 17 mount", () => {
    const part = gen.generate({ mountType: "nema17", hasBoss: false });
    // Should have 4 bolt holes in NEMA pattern
    const diffCount = (part.vcode.match(/^D /gm) || []).length;
    expect(diffCount).toBeGreaterThanOrEqual(5); // Center + 4 bolts
  });

  it("generates sensor mount", () => {
    const part = gen.generate({ mountType: "sensor" });
    expect(part.vcode).toContain("D"); // Mounting holes
  });

  it("generates adjustable mount with slots", () => {
    const part = gen.generate({ mountType: "adjustable" });
    expect(part.vcode).toContain("D"); // Slots cut out
  });
});

// ============================================================================
// New Generator Tests (Expanded IR Coverage)
// ============================================================================

describe("BallGenerator", () => {
  const gen = new BallGenerator();

  it("generates simple sphere", () => {
    const part = gen.generate({ ballType: "sphere" });
    expect(part.vcode).toMatch(/^S \d/); // Starts with sphere
    expect(part.complexity).toBe(1);
  });

  it("generates dome with intersection", () => {
    const part = gen.generate({ ballType: "dome" });
    expect(part.vcode).toContain("I"); // Intersection for dome
  });

  it("generates drilled ball", () => {
    const part = gen.generate({ ballType: "drilled" });
    expect(part.vcode).toContain("D"); // Difference for hole
    expect(part.vcode).toContain("Y"); // Cylinder for hole
  });

  it("generates handle ball", () => {
    const part = gen.generate({ ballType: "handle" });
    expect(part.vcode).toContain("U"); // Union with stem
  });
});

describe("FunnelGenerator", () => {
  const gen = new FunnelGenerator();

  it("generates simple cone", () => {
    const part = gen.generate({ funnelType: "cone" });
    expect(part.vcode).toMatch(/^K \d/); // Starts with cone
    expect(part.vcode).toContain(" 0 "); // Zero top radius for pointed
  });

  it("generates frustum", () => {
    const part = gen.generate({ funnelType: "frustum", topRadius: 10 });
    expect(part.vcode).toMatch(/^K \d/);
  });

  it("generates adapter with union", () => {
    const part = gen.generate({ funnelType: "adapter", topRadius: 10 });
    expect(part.vcode).toContain("U"); // Union with cylinder
  });

  it("generates hopper with difference", () => {
    const part = gen.generate({ funnelType: "hopper" });
    expect(part.vcode).toContain("D"); // Hollow cone
  });
});

describe("ClipGenerator", () => {
  const gen = new ClipGenerator();

  it("generates saddle with intersection", () => {
    const part = gen.generate({ clipType: "saddle" });
    expect(part.vcode).toContain("I"); // Intersection
  });

  it("generates rounded block", () => {
    const part = gen.generate({ clipType: "rounded" });
    expect(part.vcode).toContain("C"); // Cube
    expect(part.vcode).toContain("S"); // Sphere
    expect(part.vcode).toContain("I"); // Intersection
  });

  it("generates lens shape", () => {
    const part = gen.generate({ clipType: "lens" });
    expect((part.vcode.match(/S \d/g) || []).length).toBe(2); // Two spheres
    expect(part.vcode).toContain("I"); // Intersection
  });
});

describe("ScaledGenerator", () => {
  const gen = new ScaledGenerator();

  it("generates ellipse with scale", () => {
    const part = gen.generate({ scaledType: "ellipse" });
    expect(part.vcode).toContain("X"); // Scale operation
    expect(part.vcode).toContain("Y"); // Cylinder base
  });

  it("generates ellipsoid", () => {
    const part = gen.generate({ scaledType: "ellipsoid" });
    expect(part.vcode).toContain("S"); // Sphere
    expect(part.vcode).toContain("X"); // Scale
  });

  it("generates oval tube", () => {
    const part = gen.generate({ scaledType: "oval" });
    expect(part.vcode).toContain("D"); // Hollow
    expect(part.vcode).toContain("X"); // Scale
  });
});

describe("ArrayGenerator", () => {
  const gen = new ArrayGenerator();

  it("generates rail with linear pattern", () => {
    const part = gen.generate({ arrayType: "rail", count: 5 });
    expect(part.vcode).toContain("LP"); // Linear pattern
    expect(part.vcode).toContain("5 "); // Count
  });

  it("generates rack with union pattern", () => {
    const part = gen.generate({ arrayType: "rack", count: 6 });
    expect(part.vcode).toContain("LP");
    expect(part.vcode).toContain("U"); // Union teeth with base
  });

  it("generates perforated bar", () => {
    const part = gen.generate({ arrayType: "perforated", count: 4 });
    expect(part.vcode).toContain("LP");
    expect(part.vcode).toContain("D"); // Subtract holes
  });
});

describe("RadialGenerator", () => {
  const gen = new RadialGenerator();

  it("generates bolt circle with circular pattern", () => {
    const part = gen.generate({ radialType: "boltCircle", count: 6 });
    expect(part.vcode).toContain("CP"); // Circular pattern
    expect(part.vcode).toContain("6 "); // Count
  });

  it("generates spoked wheel", () => {
    const part = gen.generate({ radialType: "spoked", count: 5 });
    expect(part.vcode).toContain("CP");
    expect(part.vcode).toContain("U"); // Hub + spokes + rim
  });

  it("generates star pattern", () => {
    const part = gen.generate({ radialType: "star", count: 5 });
    expect(part.vcode).toContain("CP 2"); // Pattern of points
  });
});

describe("HollowGenerator", () => {
  const gen = new HollowGenerator();

  it("generates hollow box with shell", () => {
    const part = gen.generate({ hollowType: "box" });
    expect(part.vcode).toContain("SH"); // Shell operation
    expect(part.vcode).toContain("C"); // Cube base
  });

  it("generates tube with shell", () => {
    const part = gen.generate({ hollowType: "tube" });
    expect(part.vcode).toContain("SH");
    expect(part.vcode).toContain("Y"); // Cylinder base
  });

  it("generates dome shell", () => {
    const part = gen.generate({ hollowType: "domeShell" });
    expect(part.vcode).toContain("S"); // Sphere
    expect(part.vcode).toContain("I"); // Intersection for hemisphere
    expect(part.vcode).toContain("SH"); // Shell
  });
});

describe("ProfileGenerator", () => {
  const gen = new ProfileGenerator();

  it("generates L-channel with sketch and extrude", () => {
    const part = gen.generate({ profileType: "lChannel" });
    expect(part.vcode).toContain("SK"); // Sketch
    expect(part.vcode).toContain("L "); // Line segments
    expect(part.vcode).toContain("END");
    expect(part.vcode).toContain("E "); // Extrude
  });

  it("generates T-slot profile", () => {
    const part = gen.generate({ profileType: "tSlot" });
    expect(part.vcode).toContain("SK");
    expect(part.vcode).toContain("E ");
  });

  it("generates polygon extrusion", () => {
    const part = gen.generate({ profileType: "polygon", sides: 6 });
    expect(part.vcode).toContain("SK");
    // Should have 6 line segments
    const lineCount = (part.vcode.match(/^L /gm) || []).length;
    expect(lineCount).toBe(6);
  });
});

describe("TurnedGenerator", () => {
  const gen = new TurnedGenerator();

  it("generates bottle with sketch and revolve", () => {
    const part = gen.generate({ turnedType: "bottle" });
    expect(part.vcode).toContain("SK"); // Sketch
    expect(part.vcode).toContain("V "); // Revolve
  });

  it("generates pulley", () => {
    const part = gen.generate({ turnedType: "pulley" });
    expect(part.vcode).toContain("V ");
    expect(part.vcode).toContain("360"); // Full revolution
  });

  it("generates bowl", () => {
    const part = gen.generate({ turnedType: "bowl" });
    expect(part.vcode).toContain("SK");
    expect(part.vcode).toContain("V ");
  });
});

// ============================================================================
// Registry Tests (Updated for 16 families)
// ============================================================================

describe("Generator Registry", () => {
  it("has all expected families", () => {
    // Original families
    expect(generatorFamilies).toContain("plate");
    expect(generatorFamilies).toContain("spacer");
    expect(generatorFamilies).toContain("bracket");
    expect(generatorFamilies).toContain("flange");
    expect(generatorFamilies).toContain("shaft");
    expect(generatorFamilies).toContain("enclosure");
    expect(generatorFamilies).toContain("mount");
    // New families
    expect(generatorFamilies).toContain("ball");
    expect(generatorFamilies).toContain("funnel");
    expect(generatorFamilies).toContain("clip");
    expect(generatorFamilies).toContain("scaled");
    expect(generatorFamilies).toContain("array");
    expect(generatorFamilies).toContain("radial");
    expect(generatorFamilies).toContain("hollow");
    expect(generatorFamilies).toContain("profile");
    expect(generatorFamilies).toContain("turned");
    expect(generatorFamilies.length).toBe(16);
  });

  it("all generators produce valid IR", () => {
    for (const family of generatorFamilies) {
      const generator = generators[family];
      const part = generator.generate();

      expect(part.family).toBe(family);
      expect(part.vcode.length).toBeGreaterThan(0);
      expect(part.complexity).toBeGreaterThanOrEqual(1);
      expect(part.complexity).toBeLessThanOrEqual(5);

      // Parse should succeed
      const doc = fromVCode(part.vcode);
      expect(doc.roots.length).toBe(1);
    }
  });

  it("generateRandomPart works", () => {
    const part = generateRandomPart();
    expect(generatorFamilies).toContain(part.family);
    expect(part.vcode.length).toBeGreaterThan(0);
  });
});

describe("Validation", () => {
  it("validates correct IR", () => {
    const gen = new PlateGenerator();
    const part = gen.generate({ holePattern: "none" });

    const result = validateExample({
      text: "test",
      ir: part.vcode,
      family: part.family,
      complexity: part.complexity,
    });

    expect(result.valid).toBe(true);
  });

  it("rejects malformed IR", () => {
    const result = validateExample({
      text: "test",
      ir: "INVALID STUFF",
      family: "test",
      complexity: 1,
    });

    expect(result.valid).toBe(false);
    expect(result.error).toBe("parse_error");
  });

  it("rejects empty IR", () => {
    const result = validateExample({
      text: "test",
      ir: "",
      family: "test",
      complexity: 1,
    });

    expect(result.valid).toBe(false);
  });
});

describe("Synthetic Descriptions", () => {
  it("generates descriptions for all families", () => {
    for (const family of generatorFamilies) {
      const part = generators[family].generate();
      const desc = generateSyntheticDescription(part);

      expect(desc.length).toBeGreaterThan(0);
      expect(typeof desc).toBe("string");
    }
  });

  it("includes dimensions in plate description", () => {
    const gen = new PlateGenerator();
    const part = gen.generate({ width: 100, depth: 50, thickness: 5, holePattern: "none" });
    const desc = generateSyntheticDescription(part);

    expect(desc).toContain("100");
    expect(desc).toContain("50");
    expect(desc).toContain("5");
  });
});

describe("Random Generation Consistency", () => {
  it("generates diverse parts", () => {
    const gen = new PlateGenerator();
    const parts = Array.from({ length: 10 }, () => gen.generate());

    // Should have some variation
    const compacts = new Set(parts.map((p) => p.vcode));
    expect(compacts.size).toBeGreaterThan(1);
  });

  it("respects provided parameters", () => {
    const gen = new PlateGenerator();
    const part = gen.generate({ width: 100, depth: 60, thickness: 5 });

    expect(part.params.width).toBe(100);
    expect(part.params.depth).toBe(60);
    expect(part.params.thickness).toBe(5);
    expect(part.vcode).toContain("C 100 60 5");
  });
});

// ============================================================================
// Conversation Generator Tests
// ============================================================================

describe("Conversation Generator", () => {
  it("generates multi-turn conversation", () => {
    const gen = new PlateGenerator();
    const conv = generateConversation(gen, 3);

    expect(conv.family).toBe("plate");
    expect(conv.turns).toBeGreaterThanOrEqual(2);
    expect(conv.conversation.length).toBeGreaterThanOrEqual(4); // At least 2 turn pairs
  });

  it("alternates user and assistant roles", () => {
    const gen = new PlateGenerator();
    const conv = generateConversation(gen, 2);

    for (let i = 0; i < conv.conversation.length; i++) {
      const expectedRole = i % 2 === 0 ? "user" : "assistant";
      expect(conv.conversation[i].role).toBe(expectedRole);
    }
  });

  it("generates valid IR in assistant responses", () => {
    const gen = new PlateGenerator();
    const conv = generateConversation(gen, 2);

    // Check that assistant responses are valid IR
    for (const turn of conv.conversation) {
      if (turn.role === "assistant") {
        const doc = fromVCode(turn.content);
        expect(doc.roots.length).toBe(1);
      }
    }
  });

  it("generates multiple conversations", () => {
    const convs = generateConversations(10, {
      families: ["plate", "ball"],
      minTurns: 2,
      maxTurns: 3,
    });

    expect(convs.length).toBe(10);
    expect(convs.every(c => c.family === "plate" || c.family === "ball")).toBe(true);
  });

  it("converts to ShareGPT format", () => {
    const gen = new PlateGenerator();
    const conv = generateConversation(gen, 2);
    const sharegpt = toShareGPTFormat(conv);

    expect(sharegpt.conversations.length).toBe(conv.conversation.length);
    expect(sharegpt.conversations[0].from).toBe("human");
    expect(sharegpt.conversations[1].from).toBe("gpt");
  });
});
