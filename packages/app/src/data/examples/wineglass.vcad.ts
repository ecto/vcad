import type { Example } from "./index";
import type { Document, SketchSegment2D } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

// Wine glass created using Revolve operation
// Profile is on the XY plane (x = radial distance, y = height)
// Revolved 360 degrees around the Z axis
//
// LEARNING POINTS:
// - Revolve operation: rotates a 2D profile around an axis
// - Profile must be a closed loop for solid geometry
// - Profile X = radial distance, Profile Y = height (when revolving around Z)
// - Complex profiles can create detailed shapes (base, stem, bowl, rim)
// - Wall thickness achieved by tracing outer then inner profile edges

// Wine glass dimensions (in mm)
const baseRadius = 35;      // Radius of the foot
const baseThickness = 3;    // Thickness of the foot
const stemRadius = 3;       // Radius of the stem
const stemHeight = 60;      // Height of the stem
const bowlBottomY = baseThickness + stemHeight;  // Where bowl starts
const bowlRadius = 40;      // Maximum radius of the bowl
const bowlHeight = 70;      // Height of the bowl
const wallThickness = 2;    // Thickness of the glass walls
const rimRadius = 30;       // Radius at the rim (narrower than max)

// Helper to create line segments for the profile
// The profile traces the outer edge then inner edge (closed loop)
function createWineGlassProfile(): SketchSegment2D[] {
  const segments: SketchSegment2D[] = [];

  // We trace the profile clockwise starting from the center of the base
  // Profile is in local 2D: x = radial distance from axis, y = height

  // OUTER PROFILE (bottom to top, moving outward then up)
  // 1. Base bottom - center to outer edge
  segments.push({
    type: "Line",
    start: { x: 0, y: 0 },
    end: { x: baseRadius, y: 0 },
  });

  // 2. Base outer edge - up
  segments.push({
    type: "Line",
    start: { x: baseRadius, y: 0 },
    end: { x: baseRadius, y: baseThickness },
  });

  // 3. Base top - curve inward to stem (approximated with lines)
  segments.push({
    type: "Line",
    start: { x: baseRadius, y: baseThickness },
    end: { x: baseRadius * 0.6, y: baseThickness + 2 },
  });
  segments.push({
    type: "Line",
    start: { x: baseRadius * 0.6, y: baseThickness + 2 },
    end: { x: stemRadius + 2, y: baseThickness + 5 },
  });
  segments.push({
    type: "Line",
    start: { x: stemRadius + 2, y: baseThickness + 5 },
    end: { x: stemRadius, y: baseThickness + 8 },
  });

  // 4. Stem - straight up
  segments.push({
    type: "Line",
    start: { x: stemRadius, y: baseThickness + 8 },
    end: { x: stemRadius, y: bowlBottomY },
  });

  // 5. Bowl outer - curved shape approximated with line segments
  // Transition from stem to bowl bottom
  segments.push({
    type: "Line",
    start: { x: stemRadius, y: bowlBottomY },
    end: { x: stemRadius + 5, y: bowlBottomY + 5 },
  });

  // Bowl curve - expand outward (approximating a parabolic shape)
  const bowlSegments = 8;
  let prevX = stemRadius + 5;
  let prevY = bowlBottomY + 5;

  for (let i = 1; i <= bowlSegments; i++) {
    const t = i / bowlSegments;
    // Parabolic expansion then contraction for bowl shape
    const heightFrac = t;
    const y = bowlBottomY + 5 + (bowlHeight - 5) * heightFrac;

    // Bowl shape: expands to max then contracts to rim
    let x: number;
    if (t < 0.5) {
      // Expanding part
      const expandT = t * 2;
      x = stemRadius + 5 + (bowlRadius - stemRadius - 5) * Math.sqrt(expandT);
    } else {
      // Contracting part toward rim
      const contractT = (t - 0.5) * 2;
      x = bowlRadius - (bowlRadius - rimRadius) * contractT * contractT;
    }

    segments.push({
      type: "Line",
      start: { x: prevX, y: prevY },
      end: { x, y },
    });
    prevX = x;
    prevY = y;
  }

  // 6. Rim top edge (outer to inner)
  const rimOuterY = bowlBottomY + bowlHeight;
  segments.push({
    type: "Line",
    start: { x: rimRadius, y: rimOuterY },
    end: { x: rimRadius - wallThickness, y: rimOuterY },
  });

  // INNER PROFILE (top to bottom)
  // 7. Bowl inner - curved shape approximated with line segments
  const innerRimRadius = rimRadius - wallThickness;
  const innerBowlRadius = bowlRadius - wallThickness;
  const innerStemRadius = stemRadius - wallThickness * 0.5; // Stem is thinner at wall

  prevX = innerRimRadius;
  prevY = rimOuterY;

  for (let i = bowlSegments; i >= 1; i--) {
    const t = i / bowlSegments;
    const heightFrac = t;
    const y = bowlBottomY + 5 + (bowlHeight - 5) * heightFrac;

    let x: number;
    if (t < 0.5) {
      const expandT = t * 2;
      x = innerStemRadius + 5 + (innerBowlRadius - innerStemRadius - 5) * Math.sqrt(expandT);
    } else {
      const contractT = (t - 0.5) * 2;
      x = innerBowlRadius - (innerBowlRadius - innerRimRadius) * contractT * contractT;
    }

    segments.push({
      type: "Line",
      start: { x: prevX, y: prevY },
      end: { x, y },
    });
    prevX = x;
    prevY = y;
  }

  // Transition from bowl inner to stem cavity
  segments.push({
    type: "Line",
    start: { x: prevX, y: prevY },
    end: { x: innerStemRadius + 3, y: bowlBottomY + 5 },
  });

  // Inner cavity bottom (where wine sits)
  segments.push({
    type: "Line",
    start: { x: innerStemRadius + 3, y: bowlBottomY + 5 },
    end: { x: 0, y: bowlBottomY + 5 },
  });

  // Close the profile - back to origin along the axis
  segments.push({
    type: "Line",
    start: { x: 0, y: bowlBottomY + 5 },
    end: { x: 0, y: 0 },
  });

  return segments;
}

const document: Document = {
  version: "0.1",
  nodes: {
    // === SKETCH PROFILE ===
    // Wine glass cross-section (closed loop tracing outer then inner edges)
    // Sketch plane: XY, so sketch X = radial distance, sketch Y = height
    "1": {
      id: 1,
      name: "Wine Glass Profile",
      op: {
        type: "Sketch2D",
        origin: { x: 0, y: 0, z: 0 },
        x_dir: { x: 1, y: 0, z: 0 },
        y_dir: { x: 0, y: 0, z: 1 },
        segments: createWineGlassProfile(),
      },
    },

    // === REVOLVE OPERATION ===
    // Rotate profile 360° around Y axis to create solid
    "2": {
      id: 2,
      name: "Revolve 360° around Z",
      op: {
        type: "Revolve",
        sketch: 1,
        axis_origin: { x: 0, y: 0, z: 0 },
        axis_dir: { x: 0, y: 0, z: 1 },  // Z axis (vertical)
        angle_deg: 360,                  // Full rotation
      },
    },

    // === FINAL TRANSFORMS ===
    "3": { id: 3, name: "Scale", op: { type: "Scale", child: 2, factor: { x: 1, y: 1, z: 1 } } },
    "4": { id: 4, name: "Rotate", op: { type: "Rotate", child: 3, angles: { x: 0, y: 0, z: 0 } } },
    "5": { id: 5, name: "Wine Glass", op: { type: "Translate", child: 4, offset: { x: 0, y: 0, z: 0 } } },
  },
  materials: {
    glass: {
      name: "Glass",
      color: [0.9, 0.95, 1.0],
      metallic: 0.1,
      roughness: 0.05,
    },
  },
  part_materials: {},
  roots: [{ root: 5, material: "glass" }],
  // === ASSEMBLY STRUCTURE ===
  partDefs: {
    wineglass: {
      id: "wineglass",
      name: "Wine Glass",
      root: 5,
      defaultMaterial: "glass",
    },
  },
  instances: [
    {
      id: "wineglass-1",
      partDefId: "wineglass",
      name: "Wine Glass",
      transform: { translation: { x: 0, y: 0, z: 0 }, rotation: { x: 0, y: 0, z: 0 }, scale: { x: 1, y: 1, z: 1 } },
    },
  ],
  groundInstanceId: "wineglass-1",
};

const parts: PartInfo[] = [
  {
    id: "part-1",
    name: "Wine Glass",
    kind: "revolve",
    sketchNodeId: 1,
    revolveNodeId: 2,
    scaleNodeId: 3,
    rotateNodeId: 4,
    translateNodeId: 5,
  },
];

export const wineglassExample: Example = {
  id: "wineglass",
  name: "Wine Glass",
  file: {
    document,
    parts,
    consumedParts: {},
    nextNodeId: 6,
    nextPartNum: 2,
  },
};
