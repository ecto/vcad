/**
 * Flange generator - circular flanges with bolt patterns and hubs.
 *
 * Generates:
 * - Simple flat flanges
 * - Flanges with center hub
 * - Flanges with raised face
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
  circularHoles,
  type HolePosition,
} from "./utils.js";

export type FlangeType = "flat" | "hubbed" | "raised";

export interface FlangeParams extends PartParams {
  outerDiameter: number;
  thickness: number;
  flangeType: FlangeType;
  centerHoleDiameter: number;
  boltCount: number;
  boltCircleDiameter: number;
  boltHoleDiameter: number;
  hubOuterDiameter: number;
  hubHeight: number;
  raisedFaceDiameter: number;
  raisedFaceHeight: number;
}

export class FlangeGenerator implements PartGenerator {
  readonly family = "flange";
  readonly description = "Circular flanges with bolt patterns and hubs";

  paramDefs(): Record<string, ParamDef> {
    return {
      outerDiameter: {
        type: "number",
        range: { min: 40, max: 150, step: 5 },
        description: "Flange outer diameter (mm)",
      },
      thickness: {
        type: "number",
        range: { min: 3, max: 15, step: 1 },
        description: "Flange thickness (mm)",
      },
      flangeType: {
        type: "choice",
        choices: ["flat", "hubbed", "raised"],
        description: "Flange type",
      },
      centerHoleDiameter: {
        type: "number",
        range: { min: 10, max: 80, step: 2 },
        description: "Center bore diameter (mm)",
      },
      boltCount: {
        type: "number",
        range: { min: 3, max: 8, step: 1 },
        description: "Number of bolt holes",
      },
      boltCircleDiameter: {
        type: "number",
        range: { min: 30, max: 120, step: 5 },
        description: "Bolt circle diameter (mm)",
      },
      boltHoleDiameter: {
        type: "number",
        range: { min: 4, max: 12, step: 1 },
        description: "Bolt hole diameter (mm)",
      },
      hubOuterDiameter: {
        type: "number",
        range: { min: 20, max: 60, step: 5 },
        description: "Hub outer diameter (mm)",
      },
      hubHeight: {
        type: "number",
        range: { min: 5, max: 30, step: 2 },
        description: "Hub height (mm)",
      },
      raisedFaceDiameter: {
        type: "number",
        range: { min: 25, max: 100, step: 5 },
        description: "Raised face diameter (mm)",
      },
      raisedFaceHeight: {
        type: "number",
        range: { min: 1, max: 5, step: 0.5 },
        description: "Raised face height (mm)",
      },
    };
  }

  randomParams(): FlangeParams {
    const defs = this.paramDefs();
    const outerDiameter = randInt(defs.outerDiameter.range!);

    return {
      outerDiameter,
      thickness: randInt(defs.thickness.range!),
      flangeType: randChoice(defs.flangeType.choices!) as FlangeType,
      centerHoleDiameter: randInt({
        min: defs.centerHoleDiameter.range!.min,
        max: outerDiameter * 0.4,
      }),
      boltCount: randInt(defs.boltCount.range!),
      boltCircleDiameter: randInt({
        min: outerDiameter * 0.5,
        max: outerDiameter * 0.85,
      }),
      boltHoleDiameter: randInt(defs.boltHoleDiameter.range!),
      hubOuterDiameter: randInt({
        min: defs.hubOuterDiameter.range!.min,
        max: outerDiameter * 0.5,
      }),
      hubHeight: randInt(defs.hubHeight.range!),
      raisedFaceDiameter: randInt({
        min: outerDiameter * 0.4,
        max: outerDiameter * 0.7,
      }),
      raisedFaceHeight: randFloat(defs.raisedFaceHeight.range!, 1),
    };
  }

  generate(params?: Partial<FlangeParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as FlangeParams;

    // Ensure constraints
    p.centerHoleDiameter = Math.min(p.centerHoleDiameter, p.outerDiameter * 0.5);
    p.boltCircleDiameter = Math.max(
      p.centerHoleDiameter + p.boltHoleDiameter * 2,
      Math.min(p.boltCircleDiameter, p.outerDiameter - p.boltHoleDiameter * 2),
    );
    p.hubOuterDiameter = Math.max(
      p.centerHoleDiameter + 4,
      Math.min(p.hubOuterDiameter, p.boltCircleDiameter - p.boltHoleDiameter * 2),
    );

    const lines: string[] = [];

    // Line 0: Base flange disc
    lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.thickness)}`);
    let baseIdx = 0;

    // Add hub or raised face
    if (p.flangeType === "hubbed") {
      baseIdx = this.addHub(lines, baseIdx, p);
    } else if (p.flangeType === "raised") {
      baseIdx = this.addRaisedFace(lines, baseIdx, p);
    }

    // Add center hole
    baseIdx = this.addCenterHole(lines, baseIdx, p);

    // Add bolt holes
    baseIdx = this.addBoltHoles(lines, baseIdx, p);

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private addHub(lines: string[], baseIdx: number, p: FlangeParams): number {
    // Hub extends below the flange
    const hubIdx = lines.length;
    lines.push(`Y ${fmt(p.hubOuterDiameter / 2)} ${fmt(p.hubHeight)}`);

    // Position hub below flange
    const transIdx = lines.length;
    lines.push(`T ${hubIdx} 0 0 ${fmt(-p.hubHeight)}`);

    // Union with base
    const unionIdx = lines.length;
    lines.push(`U ${baseIdx} ${transIdx}`);

    return unionIdx;
  }

  private addRaisedFace(lines: string[], baseIdx: number, p: FlangeParams): number {
    // Raised face on top of flange
    const faceIdx = lines.length;
    lines.push(`Y ${fmt(p.raisedFaceDiameter / 2)} ${fmt(p.raisedFaceHeight)}`);

    // Position on top of flange
    const transIdx = lines.length;
    lines.push(`T ${faceIdx} 0 0 ${fmt(p.thickness)}`);

    // Union with base
    const unionIdx = lines.length;
    lines.push(`U ${baseIdx} ${transIdx}`);

    return unionIdx;
  }

  private addCenterHole(lines: string[], baseIdx: number, p: FlangeParams): number {
    // Calculate total height including hub
    let totalHeight = p.thickness;
    let zOffset = 0;
    if (p.flangeType === "hubbed") {
      totalHeight += p.hubHeight;
      zOffset = -p.hubHeight;
    } else if (p.flangeType === "raised") {
      totalHeight += p.raisedFaceHeight;
    }

    const holeIdx = lines.length;
    lines.push(`Y ${fmt(p.centerHoleDiameter / 2)} ${fmt(totalHeight + 2)}`);

    const transIdx = lines.length;
    lines.push(`T ${holeIdx} 0 0 ${fmt(zOffset - 1)}`);

    const diffIdx = lines.length;
    lines.push(`D ${baseIdx} ${transIdx}`);

    return diffIdx;
  }

  private addBoltHoles(lines: string[], baseIdx: number, p: FlangeParams): number {
    const holes = circularHoles(0, 0, p.boltCircleDiameter / 2, p.boltCount);
    let currentBase = baseIdx;

    for (const hole of holes) {
      const cylIdx = lines.length;
      lines.push(`Y ${fmt(p.boltHoleDiameter / 2)} ${fmt(p.thickness + 2)}`);

      const transIdx = lines.length;
      lines.push(`T ${cylIdx} ${fmt(hole.x)} ${fmt(hole.y)} -1`);

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private computeComplexity(p: FlangeParams): number {
    let complexity = 2; // Base flange with center hole
    if (p.flangeType !== "flat") complexity++;
    if (p.boltCount > 6) complexity++;
    return Math.min(complexity, 4);
  }
}
