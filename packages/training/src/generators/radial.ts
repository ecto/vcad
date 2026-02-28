/**
 * Radial generator - parts using the CP (CircularPattern) operation.
 *
 * Generates:
 * - Bolt circle flange
 * - Spoked wheel
 * - Fan blade pattern
 * - Circular hole array (like ventilation)
 * - Star pattern
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
} from "./utils.js";

export type RadialType = "boltCircle" | "spoked" | "fan" | "ventilation" | "star";

export interface RadialParams extends PartParams {
  radialType: RadialType;
  /** Outer diameter */
  outerDiameter: number;
  /** Inner diameter (for hollow center) */
  innerDiameter: number;
  /** Thickness */
  thickness: number;
  /** Number of pattern elements */
  count: number;
  /** Pattern angle (degrees, usually 360 for full circle) */
  angleDeg: number;
  /** Hole/element diameter for bolt circle */
  holeDiameter: number;
  /** Bolt circle diameter */
  boltCircleDiameter: number;
  /** Spoke width for spoked wheel */
  spokeWidth: number;
  /** Spoke thickness for spoked wheel */
  spokeThickness: number;
  /** Blade length for fan */
  bladeLength: number;
  /** Blade width for fan */
  bladeWidth: number;
  /** Star point length */
  starPointLength: number;
}

export class RadialGenerator implements PartGenerator {
  readonly family = "radial";
  readonly description = "Parts using CP (CircularPattern) operation";

  paramDefs(): Record<string, ParamDef> {
    return {
      radialType: {
        type: "choice",
        choices: ["boltCircle", "spoked", "fan", "ventilation", "star"],
        description: "Type of circular pattern part",
      },
      outerDiameter: {
        type: "number",
        range: { min: 40, max: 150, step: 5 },
        description: "Outer diameter (mm)",
      },
      innerDiameter: {
        type: "number",
        range: { min: 10, max: 80, step: 5 },
        description: "Inner/hub diameter (mm)",
      },
      thickness: {
        type: "number",
        range: { min: 2, max: 15, step: 1 },
        description: "Thickness (mm)",
      },
      count: {
        type: "number",
        range: { min: 3, max: 12, step: 1 },
        description: "Number of pattern elements",
      },
      angleDeg: {
        type: "number",
        range: { min: 90, max: 360, step: 30 },
        description: "Total pattern angle (degrees)",
      },
      holeDiameter: {
        type: "number",
        range: { min: 4, max: 15, step: 1 },
        description: "Bolt hole diameter (mm)",
      },
      boltCircleDiameter: {
        type: "number",
        range: { min: 30, max: 120, step: 5 },
        description: "Bolt circle diameter (mm)",
      },
      spokeWidth: {
        type: "number",
        range: { min: 4, max: 15, step: 1 },
        description: "Spoke width (mm)",
      },
      spokeThickness: {
        type: "number",
        range: { min: 3, max: 12, step: 1 },
        description: "Spoke thickness (mm)",
      },
      bladeLength: {
        type: "number",
        range: { min: 20, max: 60, step: 5 },
        description: "Fan blade length (mm)",
      },
      bladeWidth: {
        type: "number",
        range: { min: 5, max: 20, step: 2 },
        description: "Fan blade width (mm)",
      },
      starPointLength: {
        type: "number",
        range: { min: 10, max: 40, step: 2 },
        description: "Star point length (mm)",
      },
    };
  }

  randomParams(): RadialParams {
    const defs = this.paramDefs();
    const outerDiameter = randInt(defs.outerDiameter.range!);

    return {
      radialType: randChoice(defs.radialType.choices!) as RadialType,
      outerDiameter,
      innerDiameter: randInt({
        min: defs.innerDiameter.range!.min,
        max: Math.floor(outerDiameter * 0.5),
      }),
      thickness: randInt(defs.thickness.range!),
      count: randInt(defs.count.range!),
      angleDeg: randChoice([180, 270, 360]),
      holeDiameter: randInt(defs.holeDiameter.range!),
      boltCircleDiameter: randInt({
        min: Math.floor(outerDiameter * 0.5),
        max: Math.floor(outerDiameter * 0.85),
      }),
      spokeWidth: randInt(defs.spokeWidth.range!),
      spokeThickness: randInt(defs.spokeThickness.range!),
      bladeLength: randInt({
        min: defs.bladeLength.range!.min,
        max: Math.floor(outerDiameter * 0.4),
      }),
      bladeWidth: randInt(defs.bladeWidth.range!),
      starPointLength: randInt({
        min: defs.starPointLength.range!.min,
        max: Math.floor(outerDiameter * 0.3),
      }),
    };
  }

  generate(params?: Partial<RadialParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as RadialParams;

    // Ensure constraints
    p.innerDiameter = Math.min(p.innerDiameter, p.outerDiameter * 0.6);
    p.boltCircleDiameter = Math.max(
      p.innerDiameter + p.holeDiameter * 2,
      Math.min(p.boltCircleDiameter, p.outerDiameter - p.holeDiameter * 2),
    );

    const lines: string[] = [];

    switch (p.radialType) {
      case "boltCircle":
        this.generateBoltCircle(lines, p);
        break;
      case "spoked":
        this.generateSpoked(lines, p);
        break;
      case "fan":
        this.generateFan(lines, p);
        break;
      case "ventilation":
        this.generateVentilation(lines, p);
        break;
      case "star":
        this.generateStar(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateBoltCircle(lines: string[], p: RadialParams): void {
    // Flange disc with circular pattern of bolt holes
    // Line 0: Outer disc
    lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.thickness)}`);

    // Line 1: Center bore
    lines.push(`Y ${fmt(p.innerDiameter / 2)} ${fmt(p.thickness + 2)}`);

    // Line 2: Position center bore
    lines.push(`T 1 0 0 -1`);

    // Line 3: Subtract center bore
    lines.push(`D 0 2`);

    // Line 4: Single bolt hole
    lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(p.thickness + 2)}`);

    // Line 5: Position first hole on bolt circle (at radius, 0 angle)
    lines.push(`T 4 ${fmt(p.boltCircleDiameter / 2)} 0 -1`);

    // Line 6: Circular pattern of holes around Z axis
    // CP child axis_origin(x,y,z) axis_dir(x,y,z) count angle_deg
    lines.push(`CP 5 0 0 0 0 0 1 ${p.count} ${fmt(p.angleDeg)}`);

    // Line 7: Subtract hole pattern from flange
    lines.push(`D 3 6`);
  }

  private generateSpoked(lines: string[], p: RadialParams): void {
    // Spoked wheel: hub + rim + spokes
    const hubRadius = p.innerDiameter / 2;
    const rimInnerRadius = p.outerDiameter / 2 - p.spokeThickness;
    const rimOuterRadius = p.outerDiameter / 2;

    // Line 0: Hub cylinder
    lines.push(`Y ${fmt(hubRadius)} ${fmt(p.thickness)}`);

    // Line 1: Single spoke (cube from hub to rim)
    const spokeLength = rimInnerRadius - hubRadius + 2;
    lines.push(`C ${fmt(spokeLength)} ${fmt(p.spokeWidth)} ${fmt(p.spokeThickness)}`);

    // Line 2: Position spoke (starting at hub edge, centered on Y)
    lines.push(`T 1 ${fmt(hubRadius - 1)} ${fmt(-p.spokeWidth / 2)} ${fmt((p.thickness - p.spokeThickness) / 2)}`);

    // Line 3: Circular pattern of spokes
    lines.push(`CP 2 0 0 ${fmt(p.thickness / 2)} 0 0 1 ${p.count} 360`);

    // Line 4: Union hub with spokes
    lines.push(`U 0 3`);

    // Line 5: Outer rim cylinder
    lines.push(`Y ${fmt(rimOuterRadius)} ${fmt(p.thickness)}`);

    // Line 6: Inner rim cutout
    lines.push(`Y ${fmt(rimInnerRadius)} ${fmt(p.thickness + 2)}`);

    // Line 7: Position inner cutout
    lines.push(`T 6 0 0 -1`);

    // Line 8: Create rim ring
    lines.push(`D 5 7`);

    // Line 9: Union hub+spokes with rim
    lines.push(`U 4 8`);
  }

  private generateFan(lines: string[], p: RadialParams): void {
    // Fan with central hub and radial blades
    const hubRadius = p.innerDiameter / 2;

    // Line 0: Hub cylinder
    lines.push(`Y ${fmt(hubRadius)} ${fmt(p.thickness)}`);

    // Line 1: Single blade (flat rectangular)
    lines.push(`C ${fmt(p.bladeLength)} ${fmt(p.bladeWidth)} ${fmt(p.thickness / 2)}`);

    // Line 2: Position blade at hub edge
    lines.push(`T 1 ${fmt(hubRadius)} ${fmt(-p.bladeWidth / 2)} ${fmt(p.thickness / 4)}`);

    // Line 3: Circular pattern of blades
    lines.push(`CP 2 0 0 ${fmt(p.thickness / 2)} 0 0 1 ${p.count} 360`);

    // Line 4: Union hub with blades
    lines.push(`U 0 3`);
  }

  private generateVentilation(lines: string[], p: RadialParams): void {
    // Circular plate with radial pattern of ventilation holes
    // Line 0: Base disc
    lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.thickness)}`);

    // Line 1: Single ventilation hole (smaller circular hole)
    const ventRadius = p.holeDiameter / 2;
    lines.push(`Y ${fmt(ventRadius)} ${fmt(p.thickness + 2)}`);

    // Line 2: Position first hole at mid-radius
    const holeRadius = (p.outerDiameter / 2 + p.innerDiameter / 2) / 2;
    lines.push(`T 1 ${fmt(holeRadius)} 0 -1`);

    // Line 3: Circular pattern of holes
    lines.push(`CP 2 0 0 0 0 0 1 ${p.count} 360`);

    // Line 4: Subtract holes
    lines.push(`D 0 3`);
  }

  private generateStar(lines: string[], p: RadialParams): void {
    // Star shape: central disc with triangular points (approximated)
    const hubRadius = p.innerDiameter / 2;

    // Line 0: Central disc
    lines.push(`Y ${fmt(hubRadius)} ${fmt(p.thickness)}`);

    // Line 1: Star point (triangular approximated by elongated cube tapering)
    // Use a cube for simplicity
    const pointWidth = p.starPointLength * 0.3;
    lines.push(`C ${fmt(p.starPointLength)} ${fmt(pointWidth)} ${fmt(p.thickness)}`);

    // Line 2: Position point at hub edge
    lines.push(`T 1 ${fmt(hubRadius)} ${fmt(-pointWidth / 2)} 0`);

    // Line 3: Circular pattern of points
    lines.push(`CP 2 0 0 ${fmt(p.thickness / 2)} 0 0 1 ${p.count} 360`);

    // Line 4: Union hub with points
    lines.push(`U 0 3`);
  }

  private computeComplexity(p: RadialParams): number {
    if (p.count <= 4) return 2;
    if (p.count <= 8) return 3;
    return 4;
  }
}
