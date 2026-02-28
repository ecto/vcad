/**
 * Plate generator - rectangular mounting plates with hole patterns.
 *
 * Generates simple rectangular plates with various hole patterns:
 * - Corner holes (4 holes at corners)
 * - Edge holes (4 holes at edge midpoints)
 * - Grid holes (NxM grid pattern)
 * - Center hole (single centered hole)
 * - Combined patterns
 */

import type {
  PartGenerator,
  PartParams,
  ParamDef,
  GeneratedPart,
} from "./types.js";
import {
  randInt,
  randFloat,
  randChoice,
  fmt,
  cornerHoles,
  edgeHoles,
  gridHoles,
  centerHole,
  type HolePosition,
} from "./utils.js";

export type HolePattern = "none" | "corners" | "edges" | "grid" | "center" | "corners+center";

export interface PlateParams extends PartParams {
  width: number;
  depth: number;
  thickness: number;
  holePattern: HolePattern;
  holeDiameter: number;
  holeInset: number;
  gridCols: number;
  gridRows: number;
}

export class PlateGenerator implements PartGenerator {
  readonly family = "plate";
  readonly description = "Rectangular mounting plates with hole patterns";

  paramDefs(): Record<string, ParamDef> {
    return {
      width: {
        type: "number",
        range: { min: 30, max: 200, step: 5 },
        description: "Plate width (mm)",
      },
      depth: {
        type: "number",
        range: { min: 20, max: 150, step: 5 },
        description: "Plate depth (mm)",
      },
      thickness: {
        type: "number",
        range: { min: 2, max: 12, step: 0.5 },
        description: "Plate thickness (mm)",
      },
      holePattern: {
        type: "choice",
        choices: ["none", "corners", "edges", "grid", "center", "corners+center"],
        description: "Hole pattern type",
      },
      holeDiameter: {
        type: "number",
        range: { min: 2, max: 10, step: 0.5 },
        description: "Hole diameter (mm)",
      },
      holeInset: {
        type: "number",
        range: { min: 5, max: 20, step: 1 },
        description: "Distance from edge to hole center (mm)",
      },
      gridCols: {
        type: "number",
        range: { min: 2, max: 4, step: 1 },
        description: "Grid columns (for grid pattern)",
      },
      gridRows: {
        type: "number",
        range: { min: 2, max: 4, step: 1 },
        description: "Grid rows (for grid pattern)",
      },
    };
  }

  randomParams(): PlateParams {
    const defs = this.paramDefs();
    return {
      width: randInt(defs.width.range!),
      depth: randInt(defs.depth.range!),
      thickness: randFloat(defs.thickness.range!, 1),
      holePattern: randChoice(defs.holePattern.choices!) as HolePattern,
      holeDiameter: randFloat(defs.holeDiameter.range!, 1),
      holeInset: randInt(defs.holeInset.range!),
      gridCols: randInt(defs.gridCols.range!),
      gridRows: randInt(defs.gridRows.range!),
    };
  }

  generate(params?: Partial<PlateParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as PlateParams;

    // Ensure constraints are met
    p.holeInset = Math.min(p.holeInset, p.width / 3, p.depth / 3);
    p.holeDiameter = Math.min(p.holeDiameter, p.holeInset * 1.5);

    const lines: string[] = [];

    // Line 0: Base plate (cube centered at origin)
    lines.push(`C ${fmt(p.width)} ${fmt(p.depth)} ${fmt(p.thickness)}`);

    // Compute hole positions based on pattern
    const holes = this.computeHolePositions(p);

    if (holes.length > 0) {
      // Generate holes using difference operations
      // Each hole needs: cylinder, translate, difference
      let baseIdx = 0; // Index of the current base solid

      for (let i = 0; i < holes.length; i++) {
        const hole = holes[i];
        const cylIdx = lines.length;

        // Create cylinder for hole (height is 2x thickness to ensure clean cut)
        lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(p.thickness * 2)}`);

        // Translate cylinder to hole position (z offset centers it on plate)
        const transIdx = lines.length;
        lines.push(
          `T ${cylIdx} ${fmt(hole.x)} ${fmt(hole.y)} ${fmt(-p.thickness / 2)}`,
        );

        // Subtract from base
        const diffIdx = lines.length;
        lines.push(`D ${baseIdx} ${transIdx}`);
        baseIdx = diffIdx;
      }
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(holes.length),
    };
  }

  private computeHolePositions(p: PlateParams): HolePosition[] {
    switch (p.holePattern) {
      case "none":
        return [];
      case "corners":
        return cornerHoles(p.width, p.depth, p.holeInset);
      case "edges":
        return edgeHoles(p.width, p.depth, p.holeInset);
      case "grid":
        return gridHoles(p.width, p.depth, p.holeInset, p.gridCols, p.gridRows);
      case "center":
        return centerHole(p.width, p.depth);
      case "corners+center":
        return [
          ...cornerHoles(p.width, p.depth, p.holeInset),
          ...centerHole(p.width, p.depth),
        ];
      default:
        return [];
    }
  }

  private computeComplexity(holeCount: number): number {
    if (holeCount === 0) return 1;
    if (holeCount <= 4) return 2;
    return 3;
  }
}
