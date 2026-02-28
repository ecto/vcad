/**
 * Enclosure generator - hollow boxes with features.
 *
 * Generates:
 * - Simple shell boxes (hollow)
 * - Boxes with mounting standoffs
 * - Boxes with vent slots
 * - Lids/covers
 */

import type {
  PartGenerator,
  PartParams,
  ParamDef,
  GeneratedPart,
} from "./types.js";
import { randInt, randFloat, randChoice, randBool, fmt, cornerHoles } from "./utils.js";

export type EnclosureType = "box" | "lid" | "boxWithStandoffs" | "boxWithVents";

export interface EnclosureParams extends PartParams {
  enclosureType: EnclosureType;
  width: number;
  depth: number;
  height: number;
  wallThickness: number;
  hasFlange: boolean;
  flangeWidth: number;
  mountingHoleDiameter: number;
  standoffDiameter: number;
  standoffHeight: number;
  standoffInset: number;
  ventSlotWidth: number;
  ventSlotLength: number;
  ventSlotCount: number;
}

export class EnclosureGenerator implements PartGenerator {
  readonly family = "enclosure";
  readonly description = "Hollow boxes, lids, and enclosures with standoffs/vents";

  paramDefs(): Record<string, ParamDef> {
    return {
      enclosureType: {
        type: "choice",
        choices: ["box", "lid", "boxWithStandoffs", "boxWithVents"],
        description: "Enclosure type",
      },
      width: {
        type: "number",
        range: { min: 40, max: 150, step: 5 },
        description: "Width (mm)",
      },
      depth: {
        type: "number",
        range: { min: 30, max: 120, step: 5 },
        description: "Depth (mm)",
      },
      height: {
        type: "number",
        range: { min: 20, max: 80, step: 5 },
        description: "Height (mm)",
      },
      wallThickness: {
        type: "number",
        range: { min: 1.5, max: 4, step: 0.5 },
        description: "Wall thickness (mm)",
      },
      hasFlange: {
        type: "boolean",
        description: "Include mounting flange",
      },
      flangeWidth: {
        type: "number",
        range: { min: 3, max: 10, step: 1 },
        description: "Flange width beyond walls (mm)",
      },
      mountingHoleDiameter: {
        type: "number",
        range: { min: 2, max: 5, step: 0.5 },
        description: "Mounting hole diameter (mm)",
      },
      standoffDiameter: {
        type: "number",
        range: { min: 4, max: 10, step: 1 },
        description: "Standoff outer diameter (mm)",
      },
      standoffHeight: {
        type: "number",
        range: { min: 3, max: 15, step: 1 },
        description: "Standoff height (mm)",
      },
      standoffInset: {
        type: "number",
        range: { min: 5, max: 15, step: 1 },
        description: "Standoff inset from corners (mm)",
      },
      ventSlotWidth: {
        type: "number",
        range: { min: 1, max: 3, step: 0.5 },
        description: "Vent slot width (mm)",
      },
      ventSlotLength: {
        type: "number",
        range: { min: 10, max: 40, step: 5 },
        description: "Vent slot length (mm)",
      },
      ventSlotCount: {
        type: "number",
        range: { min: 3, max: 8, step: 1 },
        description: "Number of vent slots",
      },
    };
  }

  randomParams(): EnclosureParams {
    const defs = this.paramDefs();
    const height = randInt(defs.height.range!);

    return {
      enclosureType: randChoice(defs.enclosureType.choices!) as EnclosureType,
      width: randInt(defs.width.range!),
      depth: randInt(defs.depth.range!),
      height,
      wallThickness: randFloat(defs.wallThickness.range!, 1),
      hasFlange: randBool(0.5),
      flangeWidth: randInt(defs.flangeWidth.range!),
      mountingHoleDiameter: randFloat(defs.mountingHoleDiameter.range!, 1),
      standoffDiameter: randInt(defs.standoffDiameter.range!),
      standoffHeight: randInt({
        min: defs.standoffHeight.range!.min,
        max: Math.min(defs.standoffHeight.range!.max, height - 5),
      }),
      standoffInset: randInt(defs.standoffInset.range!),
      ventSlotWidth: randFloat(defs.ventSlotWidth.range!, 1),
      ventSlotLength: randInt(defs.ventSlotLength.range!),
      ventSlotCount: randInt(defs.ventSlotCount.range!),
    };
  }

  generate(params?: Partial<EnclosureParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as EnclosureParams;

    // Ensure constraints
    p.standoffInset = Math.min(p.standoffInset, p.width / 4, p.depth / 4);
    p.standoffHeight = Math.min(p.standoffHeight, p.height - p.wallThickness - 2);
    p.ventSlotLength = Math.min(p.ventSlotLength, p.width - 20);

    const lines: string[] = [];
    let baseIdx = 0;

    switch (p.enclosureType) {
      case "box":
        baseIdx = this.generateBox(lines, p);
        break;
      case "lid":
        baseIdx = this.generateLid(lines, p);
        break;
      case "boxWithStandoffs":
        baseIdx = this.generateBox(lines, p);
        baseIdx = this.addStandoffs(lines, baseIdx, p);
        break;
      case "boxWithVents":
        baseIdx = this.generateBox(lines, p);
        baseIdx = this.addVents(lines, baseIdx, p);
        break;
    }

    // Add flange if requested (for box types)
    if (p.hasFlange && p.enclosureType !== "lid") {
      baseIdx = this.addFlange(lines, baseIdx, p);
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateBox(lines: string[], p: EnclosureParams): number {
    // Outer box
    lines.push(`C ${fmt(p.width)} ${fmt(p.depth)} ${fmt(p.height)}`);

    // Inner cavity (shell operation approximated by cube difference)
    const innerIdx = lines.length;
    const innerW = p.width - 2 * p.wallThickness;
    const innerD = p.depth - 2 * p.wallThickness;
    const innerH = p.height - p.wallThickness; // Open top
    lines.push(`C ${fmt(innerW)} ${fmt(innerD)} ${fmt(innerH + 1)}`);

    // Position inner cavity
    const transIdx = lines.length;
    lines.push(`T ${innerIdx} ${fmt(p.wallThickness)} ${fmt(p.wallThickness)} ${fmt(p.wallThickness)}`);

    // Subtract inner from outer
    const diffIdx = lines.length;
    lines.push(`D 0 ${transIdx}`);

    return diffIdx;
  }

  private generateLid(lines: string[], p: EnclosureParams): number {
    // Flat lid with lip
    const lidHeight = p.wallThickness * 2;
    const lipHeight = 3;
    const lipInset = p.wallThickness + 0.5; // Clearance for fit

    // Main lid plate
    lines.push(`C ${fmt(p.width)} ${fmt(p.depth)} ${fmt(lidHeight)}`);

    // Lip (fits inside the box)
    const lipIdx = lines.length;
    const lipW = p.width - 2 * lipInset;
    const lipD = p.depth - 2 * lipInset;
    lines.push(`C ${fmt(lipW)} ${fmt(lipD)} ${fmt(lipHeight)}`);

    // Position lip below lid
    const transIdx = lines.length;
    lines.push(`T ${lipIdx} ${fmt(lipInset)} ${fmt(lipInset)} ${fmt(-lipHeight)}`);

    // Union lid and lip
    const unionIdx = lines.length;
    lines.push(`U 0 ${transIdx}`);

    return unionIdx;
  }

  private addStandoffs(lines: string[], baseIdx: number, p: EnclosureParams): number {
    const positions = cornerHoles(
      p.width - 2 * p.wallThickness,
      p.depth - 2 * p.wallThickness,
      p.standoffInset,
    );

    let currentBase = baseIdx;

    for (const pos of positions) {
      // Standoff cylinder
      const cylIdx = lines.length;
      lines.push(`Y ${fmt(p.standoffDiameter / 2)} ${fmt(p.standoffHeight)}`);

      // Position inside box
      const transIdx = lines.length;
      lines.push(
        `T ${cylIdx} ${fmt(pos.x + p.wallThickness)} ${fmt(pos.y + p.wallThickness)} ${fmt(p.wallThickness)}`,
      );

      // Union with base
      const unionIdx = lines.length;
      lines.push(`U ${currentBase} ${transIdx}`);
      currentBase = unionIdx;

      // Add screw hole in standoff
      const holeIdx = lines.length;
      lines.push(`Y ${fmt(p.mountingHoleDiameter / 2)} ${fmt(p.standoffHeight + 2)}`);

      const holeTransIdx = lines.length;
      lines.push(
        `T ${holeIdx} ${fmt(pos.x + p.wallThickness)} ${fmt(pos.y + p.wallThickness)} ${fmt(p.wallThickness - 1)}`,
      );

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${holeTransIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private addVents(lines: string[], baseIdx: number, p: EnclosureParams): number {
    // Add vent slots on one side wall
    const slotSpacing = (p.height - 10) / (p.ventSlotCount + 1);
    let currentBase = baseIdx;

    for (let i = 0; i < p.ventSlotCount; i++) {
      const slotIdx = lines.length;
      lines.push(`C ${fmt(p.ventSlotLength)} ${fmt(p.wallThickness * 2)} ${fmt(p.ventSlotWidth)}`);

      // Position on front wall
      const transIdx = lines.length;
      const xPos = (p.width - p.ventSlotLength) / 2;
      const zPos = slotSpacing * (i + 1) + 5;
      lines.push(`T ${slotIdx} ${fmt(xPos)} ${fmt(-p.wallThickness / 2)} ${fmt(zPos)}`);

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private addFlange(lines: string[], baseIdx: number, p: EnclosureParams): number {
    // Flange around the top edge
    const flangeW = p.width + 2 * p.flangeWidth;
    const flangeD = p.depth + 2 * p.flangeWidth;

    const flangeIdx = lines.length;
    lines.push(`C ${fmt(flangeW)} ${fmt(flangeD)} ${fmt(p.wallThickness)}`);

    // Position at top
    const transIdx = lines.length;
    lines.push(`T ${flangeIdx} ${fmt(-p.flangeWidth)} ${fmt(-p.flangeWidth)} ${fmt(p.height - p.wallThickness)}`);

    // Union with base
    const unionIdx = lines.length;
    lines.push(`U ${baseIdx} ${transIdx}`);

    // Add mounting holes in flange corners
    let currentBase = unionIdx;
    const holePositions = cornerHoles(flangeW, flangeD, p.flangeWidth * 0.6);

    for (const pos of holePositions) {
      const holeIdx = lines.length;
      lines.push(`Y ${fmt(p.mountingHoleDiameter / 2)} ${fmt(p.wallThickness * 2)}`);

      const holeTransIdx = lines.length;
      lines.push(
        `T ${holeIdx} ${fmt(pos.x - p.flangeWidth)} ${fmt(pos.y - p.flangeWidth)} ${fmt(p.height - p.wallThickness - 0.5)}`,
      );

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${holeTransIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private computeComplexity(p: EnclosureParams): number {
    let complexity = 3; // Base shell is moderately complex
    if (p.enclosureType === "boxWithStandoffs") complexity = 4;
    if (p.enclosureType === "boxWithVents") complexity = 4;
    if (p.hasFlange) complexity++;
    return Math.min(complexity, 5);
  }
}
