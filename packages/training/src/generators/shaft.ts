/**
 * Shaft generator - stepped shafts and cylinders with features.
 *
 * Generates:
 * - Simple shafts (single diameter)
 * - Stepped shafts (2-3 diameter steps)
 * - Shafts with keyway slots
 * - Shafts with center holes
 */

import type {
  PartGenerator,
  PartParams,
  ParamDef,
  GeneratedPart,
} from "./types.js";
import { randInt, randFloat, randChoice, randBool, fmt } from "./utils.js";

export type ShaftType = "simple" | "stepped2" | "stepped3";

export interface ShaftParams extends PartParams {
  shaftType: ShaftType;
  diameter1: number;
  length1: number;
  diameter2: number;
  length2: number;
  diameter3: number;
  length3: number;
  hasCenterHole: boolean;
  centerHoleDiameter: number;
  hasKeyway: boolean;
  keywayWidth: number;
  keywayDepth: number;
}

export class ShaftGenerator implements PartGenerator {
  readonly family = "shaft";
  readonly description = "Stepped shafts with optional keyways and center holes";

  paramDefs(): Record<string, ParamDef> {
    return {
      shaftType: {
        type: "choice",
        choices: ["simple", "stepped2", "stepped3"],
        description: "Shaft type (number of steps)",
      },
      diameter1: {
        type: "number",
        range: { min: 8, max: 40, step: 2 },
        description: "First section diameter (mm)",
      },
      length1: {
        type: "number",
        range: { min: 10, max: 60, step: 5 },
        description: "First section length (mm)",
      },
      diameter2: {
        type: "number",
        range: { min: 10, max: 50, step: 2 },
        description: "Second section diameter (mm)",
      },
      length2: {
        type: "number",
        range: { min: 15, max: 80, step: 5 },
        description: "Second section length (mm)",
      },
      diameter3: {
        type: "number",
        range: { min: 8, max: 40, step: 2 },
        description: "Third section diameter (mm)",
      },
      length3: {
        type: "number",
        range: { min: 10, max: 60, step: 5 },
        description: "Third section length (mm)",
      },
      hasCenterHole: {
        type: "boolean",
        description: "Include center hole (hollow shaft)",
      },
      centerHoleDiameter: {
        type: "number",
        range: { min: 3, max: 20, step: 1 },
        description: "Center hole diameter (mm)",
      },
      hasKeyway: {
        type: "boolean",
        description: "Include keyway slot",
      },
      keywayWidth: {
        type: "number",
        range: { min: 2, max: 10, step: 1 },
        description: "Keyway width (mm)",
      },
      keywayDepth: {
        type: "number",
        range: { min: 1, max: 5, step: 0.5 },
        description: "Keyway depth (mm)",
      },
    };
  }

  randomParams(): ShaftParams {
    const defs = this.paramDefs();
    const shaftType = randChoice(defs.shaftType.choices!) as ShaftType;
    const diameter1 = randInt(defs.diameter1.range!);

    // For stepped shafts, middle section is typically larger
    const diameter2 =
      shaftType === "simple"
        ? diameter1
        : randInt({ min: diameter1, max: Math.min(diameter1 * 1.5, 50) });
    const diameter3 =
      shaftType === "stepped3"
        ? randInt({ min: defs.diameter3.range!.min, max: diameter2 })
        : diameter1;

    return {
      shaftType,
      diameter1,
      length1: randInt(defs.length1.range!),
      diameter2,
      length2: randInt(defs.length2.range!),
      diameter3,
      length3: randInt(defs.length3.range!),
      hasCenterHole: randBool(0.3),
      centerHoleDiameter: randInt({
        min: defs.centerHoleDiameter.range!.min,
        max: Math.min(diameter1 * 0.4, defs.centerHoleDiameter.range!.max),
      }),
      hasKeyway: randBool(0.3),
      keywayWidth: randInt(defs.keywayWidth.range!),
      keywayDepth: randFloat(defs.keywayDepth.range!, 1),
    };
  }

  generate(params?: Partial<ShaftParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as ShaftParams;

    // Ensure constraints
    const minDiameter = Math.min(p.diameter1, p.diameter2, p.diameter3);
    p.centerHoleDiameter = Math.min(p.centerHoleDiameter, minDiameter * 0.6);
    p.keywayWidth = Math.min(p.keywayWidth, minDiameter * 0.4);
    p.keywayDepth = Math.min(p.keywayDepth, minDiameter * 0.2);

    const lines: string[] = [];
    let baseIdx = 0;

    switch (p.shaftType) {
      case "simple":
        lines.push(`Y ${fmt(p.diameter1 / 2)} ${fmt(p.length1)}`);
        break;

      case "stepped2":
        baseIdx = this.generateSteppedShaft2(lines, p);
        break;

      case "stepped3":
        baseIdx = this.generateSteppedShaft3(lines, p);
        break;
    }

    // Add center hole if requested
    if (p.hasCenterHole) {
      baseIdx = this.addCenterHole(lines, baseIdx, p);
    }

    // Add keyway if requested
    if (p.hasKeyway) {
      baseIdx = this.addKeyway(lines, baseIdx, p);
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateSteppedShaft2(lines: string[], p: ShaftParams): number {
    // Section 1 (smaller, at bottom)
    lines.push(`Y ${fmt(p.diameter1 / 2)} ${fmt(p.length1)}`);

    // Section 2 (larger, on top)
    const sec2Idx = lines.length;
    lines.push(`Y ${fmt(p.diameter2 / 2)} ${fmt(p.length2)}`);

    // Translate section 2 on top of section 1
    const transIdx = lines.length;
    lines.push(`T ${sec2Idx} 0 0 ${fmt(p.length1)}`);

    // Union
    const unionIdx = lines.length;
    lines.push(`U 0 ${transIdx}`);

    return unionIdx;
  }

  private generateSteppedShaft3(lines: string[], p: ShaftParams): number {
    // Section 1 (at bottom)
    lines.push(`Y ${fmt(p.diameter1 / 2)} ${fmt(p.length1)}`);

    // Section 2 (middle, typically larger)
    const sec2Idx = lines.length;
    lines.push(`Y ${fmt(p.diameter2 / 2)} ${fmt(p.length2)}`);
    const trans2Idx = lines.length;
    lines.push(`T ${sec2Idx} 0 0 ${fmt(p.length1)}`);

    // Union sections 1 and 2
    const union1Idx = lines.length;
    lines.push(`U 0 ${trans2Idx}`);

    // Section 3 (top)
    const sec3Idx = lines.length;
    lines.push(`Y ${fmt(p.diameter3 / 2)} ${fmt(p.length3)}`);
    const trans3Idx = lines.length;
    lines.push(`T ${sec3Idx} 0 0 ${fmt(p.length1 + p.length2)}`);

    // Final union
    const union2Idx = lines.length;
    lines.push(`U ${union1Idx} ${trans3Idx}`);

    return union2Idx;
  }

  private addCenterHole(lines: string[], baseIdx: number, p: ShaftParams): number {
    const totalLength = this.getTotalLength(p);

    const holeIdx = lines.length;
    lines.push(`Y ${fmt(p.centerHoleDiameter / 2)} ${fmt(totalLength + 2)}`);

    const transIdx = lines.length;
    lines.push(`T ${holeIdx} 0 0 -1`);

    const diffIdx = lines.length;
    lines.push(`D ${baseIdx} ${transIdx}`);

    return diffIdx;
  }

  private addKeyway(lines: string[], baseIdx: number, p: ShaftParams): number {
    // Keyway is a rectangular slot cut into the shaft
    // Runs along a portion of section 2 (or full length for simple)
    const keywayLength = p.shaftType === "simple" ? p.length1 * 0.6 : p.length2 * 0.8;
    const keywayStart = p.shaftType === "simple" ? p.length1 * 0.2 : p.length1;

    const slotIdx = lines.length;
    lines.push(`C ${fmt(p.keywayWidth)} ${fmt(p.diameter2)} ${fmt(keywayLength)}`);

    // Position at top of shaft (radially) and along length
    const transIdx = lines.length;
    const radialOffset = p.diameter2 / 2 - p.keywayDepth;
    lines.push(
      `T ${slotIdx} ${fmt(-p.keywayWidth / 2)} ${fmt(radialOffset)} ${fmt(keywayStart)}`,
    );

    const diffIdx = lines.length;
    lines.push(`D ${baseIdx} ${transIdx}`);

    return diffIdx;
  }

  private getTotalLength(p: ShaftParams): number {
    switch (p.shaftType) {
      case "simple":
        return p.length1;
      case "stepped2":
        return p.length1 + p.length2;
      case "stepped3":
        return p.length1 + p.length2 + p.length3;
    }
  }

  private computeComplexity(p: ShaftParams): number {
    let complexity = 1;
    if (p.shaftType === "stepped2") complexity = 2;
    if (p.shaftType === "stepped3") complexity = 3;
    if (p.hasCenterHole) complexity++;
    if (p.hasKeyway) complexity++;
    return Math.min(complexity, 5);
  }
}
