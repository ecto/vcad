/**
 * Bracket generator - L-brackets, gusseted brackets, and slotted brackets.
 *
 * Generates structural brackets:
 * - Simple L-brackets
 * - Gusseted L-brackets (with triangular reinforcement)
 * - Slotted brackets (with mounting slots)
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
  randBool,
  fmt,
  cornerHoles,
  type HolePosition,
} from "./utils.js";

export type BracketType = "simple" | "gusseted" | "slotted";

export interface BracketParams extends PartParams {
  legWidth: number;
  leg1Length: number;
  leg2Length: number;
  thickness: number;
  bracketType: BracketType;
  hasHoles: boolean;
  holeDiameter: number;
  holeInset: number;
  gussetSize: number;
}

export class BracketGenerator implements PartGenerator {
  readonly family = "bracket";
  readonly description = "L-brackets, gusseted brackets, and slotted brackets";

  paramDefs(): Record<string, ParamDef> {
    return {
      legWidth: {
        type: "number",
        range: { min: 15, max: 50, step: 5 },
        description: "Width of bracket legs (mm)",
      },
      leg1Length: {
        type: "number",
        range: { min: 20, max: 80, step: 5 },
        description: "Length of first leg (mm)",
      },
      leg2Length: {
        type: "number",
        range: { min: 20, max: 80, step: 5 },
        description: "Length of second leg (mm)",
      },
      thickness: {
        type: "number",
        range: { min: 2, max: 8, step: 0.5 },
        description: "Material thickness (mm)",
      },
      bracketType: {
        type: "choice",
        choices: ["simple", "gusseted", "slotted"],
        description: "Bracket type",
      },
      hasHoles: {
        type: "boolean",
        description: "Include mounting holes",
      },
      holeDiameter: {
        type: "number",
        range: { min: 3, max: 8, step: 0.5 },
        description: "Mounting hole diameter (mm)",
      },
      holeInset: {
        type: "number",
        range: { min: 5, max: 15, step: 1 },
        description: "Distance from edge to hole center (mm)",
      },
      gussetSize: {
        type: "number",
        range: { min: 10, max: 30, step: 5 },
        description: "Gusset triangle size (mm)",
      },
    };
  }

  randomParams(): BracketParams {
    const defs = this.paramDefs();
    return {
      legWidth: randInt(defs.legWidth.range!),
      leg1Length: randInt(defs.leg1Length.range!),
      leg2Length: randInt(defs.leg2Length.range!),
      thickness: randFloat(defs.thickness.range!, 1),
      bracketType: randChoice(defs.bracketType.choices!) as BracketType,
      hasHoles: randBool(0.7),
      holeDiameter: randFloat(defs.holeDiameter.range!, 1),
      holeInset: randInt(defs.holeInset.range!),
      gussetSize: randInt(defs.gussetSize.range!),
    };
  }

  generate(params?: Partial<BracketParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as BracketParams;

    // Ensure constraints
    p.holeInset = Math.min(p.holeInset, p.legWidth / 3);
    p.holeDiameter = Math.min(p.holeDiameter, p.holeInset * 1.5);
    p.gussetSize = Math.min(p.gussetSize, p.leg1Length - p.thickness, p.leg2Length - p.thickness);

    const lines: string[] = [];

    // Create L-bracket base (two cubes)
    // Leg 1: horizontal along X
    lines.push(`C ${fmt(p.leg1Length)} ${fmt(p.legWidth)} ${fmt(p.thickness)}`);

    // Leg 2: vertical along Z, positioned at the end of leg 1
    const leg2Idx = lines.length;
    lines.push(`C ${fmt(p.thickness)} ${fmt(p.legWidth)} ${fmt(p.leg2Length)}`);

    // Translate leg 2 to connect at corner
    const leg2TransIdx = lines.length;
    lines.push(`T ${leg2Idx} ${fmt(p.leg1Length - p.thickness)} 0 0`);

    // Union the two legs
    let baseIdx = lines.length;
    lines.push(`U 0 ${leg2TransIdx}`);

    // Add gusset if requested
    if (p.bracketType === "gusseted") {
      baseIdx = this.addGusset(lines, baseIdx, p);
    }

    // Add holes if requested
    if (p.hasHoles) {
      baseIdx = this.addHoles(lines, baseIdx, p);
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private addGusset(lines: string[], baseIdx: number, p: BracketParams): number {
    // Gusset is a triangular prism approximated by a rotated cube
    // For simplicity, use a cube rotated 45 degrees and intersected

    // Create gusset block
    const gussetIdx = lines.length;
    lines.push(`C ${fmt(p.gussetSize)} ${fmt(p.legWidth)} ${fmt(p.gussetSize)}`);

    // Translate to corner position (inside the L)
    const transIdx = lines.length;
    lines.push(`T ${gussetIdx} ${fmt(p.leg1Length - p.thickness)} 0 ${fmt(p.thickness)}`);

    // Union gusset with base
    const unionIdx = lines.length;
    lines.push(`U ${baseIdx} ${transIdx}`);

    return unionIdx;
  }

  private addHoles(lines: string[], baseIdx: number, p: BracketParams): number {
    let currentBase = baseIdx;

    // Add holes to leg 1 (horizontal)
    const leg1Holes = this.computeLegHoles(p.leg1Length, p.legWidth, p.holeInset);
    for (const hole of leg1Holes) {
      const cylIdx = lines.length;
      lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(p.thickness * 2)}`);

      const transIdx = lines.length;
      lines.push(`T ${cylIdx} ${fmt(hole.x)} ${fmt(hole.y)} ${fmt(-p.thickness / 2)}`);

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    // Add holes to leg 2 (vertical) - holes go through X direction
    const leg2Holes = this.computeLegHoles(p.leg2Length, p.legWidth, p.holeInset);
    for (const hole of leg2Holes) {
      const cylIdx = lines.length;
      lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(p.thickness * 2)}`);

      // Rotate cylinder to align with X axis
      const rotIdx = lines.length;
      lines.push(`R ${cylIdx} 0 90 0`);

      // Position on leg 2 (centered in thickness)
      const transIdx = lines.length;
      lines.push(
        `T ${rotIdx} ${fmt(p.leg1Length - p.thickness / 2)} ${fmt(hole.y)} ${fmt(hole.x)}`,
      );

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private computeLegHoles(
    legLength: number,
    legWidth: number,
    inset: number,
  ): HolePosition[] {
    // Two holes along the leg
    const usableLength = legLength - 2 * inset;
    if (usableLength < inset) {
      // Just one hole in center
      return [{ x: legLength / 2, y: legWidth / 2 }];
    }

    return [
      { x: inset, y: legWidth / 2 },
      { x: legLength - inset, y: legWidth / 2 },
    ];
  }

  private computeComplexity(p: BracketParams): number {
    let complexity = 2; // Base L-bracket
    if (p.bracketType === "gusseted") complexity++;
    if (p.hasHoles) complexity++;
    return Math.min(complexity, 4);
  }
}
