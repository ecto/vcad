/**
 * Array generator - parts using the LP (LinearPattern) operation.
 *
 * Generates:
 * - Rail with evenly spaced holes
 * - DIN rail mounting strip
 * - Toothed rack segment
 * - Perforated bar
 * - Slotted plate
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
} from "./utils.js";

export type ArrayType = "rail" | "din" | "rack" | "perforated" | "slotted";

export interface ArrayParams extends PartParams {
  arrayType: ArrayType;
  /** Base bar/plate length */
  length: number;
  /** Width */
  width: number;
  /** Thickness/height */
  thickness: number;
  /** Number of pattern repeats */
  count: number;
  /** Spacing between pattern elements */
  spacing: number;
  /** Hole diameter for rail/perforated */
  holeDiameter: number;
  /** Tooth height for rack */
  toothHeight: number;
  /** Tooth width for rack */
  toothWidth: number;
  /** Slot width for slotted */
  slotWidth: number;
  /** Slot length for slotted */
  slotLength: number;
}

export class ArrayGenerator implements PartGenerator {
  readonly family = "array";
  readonly description = "Parts using LP (LinearPattern) operation";

  paramDefs(): Record<string, ParamDef> {
    return {
      arrayType: {
        type: "choice",
        choices: ["rail", "din", "rack", "perforated", "slotted"],
        description: "Type of linear pattern part",
      },
      length: {
        type: "number",
        range: { min: 50, max: 200, step: 10 },
        description: "Base length (mm)",
      },
      width: {
        type: "number",
        range: { min: 15, max: 50, step: 5 },
        description: "Width (mm)",
      },
      thickness: {
        type: "number",
        range: { min: 2, max: 10, step: 1 },
        description: "Thickness (mm)",
      },
      count: {
        type: "number",
        range: { min: 3, max: 12, step: 1 },
        description: "Number of pattern repeats",
      },
      spacing: {
        type: "number",
        range: { min: 8, max: 30, step: 2 },
        description: "Spacing between elements (mm)",
      },
      holeDiameter: {
        type: "number",
        range: { min: 3, max: 10, step: 0.5 },
        description: "Hole diameter (mm)",
      },
      toothHeight: {
        type: "number",
        range: { min: 2, max: 8, step: 1 },
        description: "Tooth height for rack (mm)",
      },
      toothWidth: {
        type: "number",
        range: { min: 3, max: 10, step: 1 },
        description: "Tooth width for rack (mm)",
      },
      slotWidth: {
        type: "number",
        range: { min: 4, max: 12, step: 1 },
        description: "Slot width (mm)",
      },
      slotLength: {
        type: "number",
        range: { min: 10, max: 30, step: 2 },
        description: "Slot length (mm)",
      },
    };
  }

  randomParams(): ArrayParams {
    const defs = this.paramDefs();
    const count = randInt(defs.count.range!);
    const spacing = randInt(defs.spacing.range!);

    // Calculate appropriate length based on count and spacing
    const minLength = (count - 1) * spacing + 20;
    const length = Math.max(minLength, randInt(defs.length.range!));

    return {
      arrayType: randChoice(defs.arrayType.choices!) as ArrayType,
      length,
      width: randInt(defs.width.range!),
      thickness: randInt(defs.thickness.range!),
      count,
      spacing,
      holeDiameter: randFloat(defs.holeDiameter.range!, 1),
      toothHeight: randInt(defs.toothHeight.range!),
      toothWidth: randInt(defs.toothWidth.range!),
      slotWidth: randInt(defs.slotWidth.range!),
      slotLength: randInt(defs.slotLength.range!),
    };
  }

  generate(params?: Partial<ArrayParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as ArrayParams;

    // Ensure constraints
    const totalPatternLength = (p.count - 1) * p.spacing;
    if (totalPatternLength > p.length - 10) {
      p.length = totalPatternLength + 20;
    }

    const lines: string[] = [];

    switch (p.arrayType) {
      case "rail":
        this.generateRail(lines, p);
        break;
      case "din":
        this.generateDin(lines, p);
        break;
      case "rack":
        this.generateRack(lines, p);
        break;
      case "perforated":
        this.generatePerforated(lines, p);
        break;
      case "slotted":
        this.generateSlotted(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateRail(lines: string[], p: ArrayParams): void {
    // Rail with evenly spaced mounting holes along centerline
    // Line 0: Base rail bar
    lines.push(`C ${fmt(p.length)} ${fmt(p.width)} ${fmt(p.thickness)}`);

    // Line 1: Single hole cylinder
    lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(p.thickness + 2)}`);

    // Line 2: Position first hole (centered on width, inset from end)
    const startX = (p.length - (p.count - 1) * p.spacing) / 2;
    lines.push(`T 1 ${fmt(startX)} ${fmt(p.width / 2)} -1`);

    // Line 3: Linear pattern of holes along X axis
    lines.push(`LP 2 1 0 0 ${p.count} ${fmt(p.spacing)}`);

    // Line 4: Subtract pattern from base
    lines.push(`D 0 3`);
  }

  private generateDin(lines: string[], p: ArrayParams): void {
    // DIN rail mounting strip with characteristic slots
    // DIN rails have an asymmetric profile, but we simplify to C-channel with slots

    // Line 0: Base rail
    lines.push(`C ${fmt(p.length)} ${fmt(p.width)} ${fmt(p.thickness)}`);

    // Line 1: Slot cutout (rectangular hole)
    const slotW = p.slotWidth;
    const slotL = p.width * 0.6;
    lines.push(`C ${fmt(slotW)} ${fmt(slotL)} ${fmt(p.thickness + 2)}`);

    // Line 2: Position first slot
    const startX = (p.length - (p.count - 1) * p.spacing) / 2 - slotW / 2;
    const slotY = (p.width - slotL) / 2;
    lines.push(`T 1 ${fmt(startX)} ${fmt(slotY)} -1`);

    // Line 3: Linear pattern of slots
    lines.push(`LP 2 1 0 0 ${p.count} ${fmt(p.spacing)}`);

    // Line 4: Subtract pattern
    lines.push(`D 0 3`);
  }

  private generateRack(lines: string[], p: ArrayParams): void {
    // Toothed rack segment for rack-and-pinion
    // Base bar with teeth added on one edge

    // Line 0: Base bar
    lines.push(`C ${fmt(p.length)} ${fmt(p.width)} ${fmt(p.thickness)}`);

    // Line 1: Single tooth (triangular approximated by cube for simplicity)
    // In real rack, teeth would be involute, but cube teeth work for training data
    lines.push(`C ${fmt(p.toothWidth)} ${fmt(p.toothHeight)} ${fmt(p.thickness)}`);

    // Line 2: Position first tooth at edge of bar
    const startX = (p.length - (p.count - 1) * p.spacing) / 2 - p.toothWidth / 2;
    lines.push(`T 1 ${fmt(startX)} ${fmt(p.width)} 0`);

    // Line 3: Linear pattern of teeth
    lines.push(`LP 2 1 0 0 ${p.count} ${fmt(p.spacing)}`);

    // Line 4: Union teeth with base
    lines.push(`U 0 3`);
  }

  private generatePerforated(lines: string[], p: ArrayParams): void {
    // Perforated bar with 2D grid of holes (uses LP for one row)
    // Single row of holes along length

    // Line 0: Base bar
    lines.push(`C ${fmt(p.length)} ${fmt(p.width)} ${fmt(p.thickness)}`);

    // Line 1: Single hole
    lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(p.thickness + 2)}`);

    // Line 2: Position first hole
    const startX = (p.length - (p.count - 1) * p.spacing) / 2;
    lines.push(`T 1 ${fmt(startX)} ${fmt(p.width / 2)} -1`);

    // Line 3: Linear pattern along X
    lines.push(`LP 2 1 0 0 ${p.count} ${fmt(p.spacing)}`);

    // Line 4: Subtract holes
    lines.push(`D 0 3`);
  }

  private generateSlotted(lines: string[], p: ArrayParams): void {
    // Plate with evenly spaced slots (elongated holes)

    // Line 0: Base plate
    lines.push(`C ${fmt(p.length)} ${fmt(p.width)} ${fmt(p.thickness)}`);

    // Line 1: Slot (elongated hole using cube)
    lines.push(`C ${fmt(p.slotWidth)} ${fmt(p.slotLength)} ${fmt(p.thickness + 2)}`);

    // Line 2: Position first slot
    const startX = (p.length - (p.count - 1) * p.spacing) / 2 - p.slotWidth / 2;
    const slotY = (p.width - p.slotLength) / 2;
    lines.push(`T 1 ${fmt(startX)} ${fmt(slotY)} -1`);

    // Line 3: Linear pattern of slots
    lines.push(`LP 2 1 0 0 ${p.count} ${fmt(p.spacing)}`);

    // Line 4: Subtract pattern
    lines.push(`D 0 3`);
  }

  private computeComplexity(p: ArrayParams): number {
    if (p.count <= 4) return 2;
    if (p.count <= 8) return 3;
    return 4;
  }
}
