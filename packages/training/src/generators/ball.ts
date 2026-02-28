/**
 * Ball generator - spherical parts using the S (Sphere) primitive.
 *
 * Generates:
 * - Simple sphere (ball bearing, marble)
 * - Dome (hemisphere)
 * - Ball with mounting hole
 * - Knob with flat bottom
 * - Handle ball
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

export type BallType = "sphere" | "dome" | "drilled" | "knob" | "handle";

export interface BallParams extends PartParams {
  radius: number;
  ballType: BallType;
  /** For drilled: hole diameter */
  holeDiameter: number;
  /** For dome/knob: flat cut depth ratio (0.1-0.5 of radius) */
  flatRatio: number;
  /** For handle: stem diameter */
  stemDiameter: number;
  /** For handle: stem length */
  stemLength: number;
  /** For knob: has threaded insert hole */
  hasInsert: boolean;
  /** Insert hole diameter */
  insertDiameter: number;
  /** Insert hole depth */
  insertDepth: number;
}

export class BallGenerator implements PartGenerator {
  readonly family = "ball";
  readonly description = "Spherical parts using S (Sphere) primitive";

  paramDefs(): Record<string, ParamDef> {
    return {
      radius: {
        type: "number",
        range: { min: 5, max: 50, step: 1 },
        description: "Ball radius (mm)",
      },
      ballType: {
        type: "choice",
        choices: ["sphere", "dome", "drilled", "knob", "handle"],
        description: "Type of spherical part",
      },
      holeDiameter: {
        type: "number",
        range: { min: 2, max: 15, step: 0.5 },
        description: "Through-hole diameter for drilled type (mm)",
      },
      flatRatio: {
        type: "number",
        range: { min: 0.1, max: 0.5, step: 0.05 },
        description: "Flat cut depth as ratio of radius",
      },
      stemDiameter: {
        type: "number",
        range: { min: 4, max: 20, step: 1 },
        description: "Handle stem diameter (mm)",
      },
      stemLength: {
        type: "number",
        range: { min: 10, max: 50, step: 2 },
        description: "Handle stem length (mm)",
      },
      hasInsert: {
        type: "boolean",
        description: "Whether knob has threaded insert hole",
      },
      insertDiameter: {
        type: "number",
        range: { min: 3, max: 10, step: 0.5 },
        description: "Threaded insert hole diameter (mm)",
      },
      insertDepth: {
        type: "number",
        range: { min: 5, max: 25, step: 1 },
        description: "Threaded insert hole depth (mm)",
      },
    };
  }

  randomParams(): BallParams {
    const defs = this.paramDefs();
    const radius = randInt(defs.radius.range!);

    return {
      radius,
      ballType: randChoice(defs.ballType.choices!) as BallType,
      holeDiameter: randFloat({
        min: defs.holeDiameter.range!.min,
        max: Math.min(defs.holeDiameter.range!.max, radius * 0.6),
      }, 1),
      flatRatio: randFloat(defs.flatRatio.range!, 2),
      stemDiameter: randInt({
        min: defs.stemDiameter.range!.min,
        max: Math.min(defs.stemDiameter.range!.max, radius * 0.8),
      }),
      stemLength: randInt(defs.stemLength.range!),
      hasInsert: randBool(0.6),
      insertDiameter: randFloat(defs.insertDiameter.range!, 1),
      insertDepth: randInt({
        min: defs.insertDepth.range!.min,
        max: Math.min(defs.insertDepth.range!.max, radius * 1.5),
      }),
    };
  }

  generate(params?: Partial<BallParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as BallParams;

    // Ensure constraints
    p.holeDiameter = Math.min(p.holeDiameter, p.radius * 0.8);
    p.stemDiameter = Math.min(p.stemDiameter, p.radius * 0.9);
    p.insertDiameter = Math.min(p.insertDiameter, p.radius * 0.6);
    p.insertDepth = Math.min(p.insertDepth, p.radius * 1.8);

    const lines: string[] = [];

    switch (p.ballType) {
      case "sphere":
        this.generateSphere(lines, p);
        break;
      case "dome":
        this.generateDome(lines, p);
        break;
      case "drilled":
        this.generateDrilled(lines, p);
        break;
      case "knob":
        this.generateKnob(lines, p);
        break;
      case "handle":
        this.generateHandle(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateSphere(lines: string[], p: BallParams): void {
    // Simple sphere - just S primitive
    lines.push(`S ${fmt(p.radius)}`);
  }

  private generateDome(lines: string[], p: BallParams): void {
    // Sphere intersected with cube to create dome
    const flatCut = p.radius * p.flatRatio;

    // Line 0: Sphere
    lines.push(`S ${fmt(p.radius)}`);

    // Line 1: Cube for intersection (positioned to cut off bottom)
    // Cube is 2*radius wide/deep, positioned to keep top hemisphere
    const cubeSize = p.radius * 2.2;
    lines.push(`C ${fmt(cubeSize)} ${fmt(cubeSize)} ${fmt(p.radius + flatCut)}`);

    // Line 2: Position cube so bottom aligns with desired cut
    lines.push(`T 1 ${fmt(-cubeSize / 2)} ${fmt(-cubeSize / 2)} ${fmt(-flatCut)}`);

    // Line 3: Intersection to create dome
    lines.push(`I 0 2`);
  }

  private generateDrilled(lines: string[], p: BallParams): void {
    // Sphere with through-hole
    // Line 0: Sphere
    lines.push(`S ${fmt(p.radius)}`);

    // Line 1: Cylinder for hole (through the center, along Z axis)
    const holeLength = p.radius * 3; // Ensure it goes through
    lines.push(`Y ${fmt(p.holeDiameter / 2)} ${fmt(holeLength)}`);

    // Line 2: Center the hole cylinder
    lines.push(`T 1 0 0 ${fmt(-holeLength / 2)}`);

    // Line 3: Subtract hole
    lines.push(`D 0 2`);
  }

  private generateKnob(lines: string[], p: BallParams): void {
    // Sphere with flat bottom (for knob) and optional threaded insert
    const flatCut = p.radius * p.flatRatio;

    // Line 0: Sphere
    lines.push(`S ${fmt(p.radius)}`);

    // Line 1: Cube for intersection (cuts off bottom to create flat base)
    const cubeSize = p.radius * 2.2;
    lines.push(`C ${fmt(cubeSize)} ${fmt(cubeSize)} ${fmt(p.radius + flatCut)}`);

    // Line 2: Position cube
    lines.push(`T 1 ${fmt(-cubeSize / 2)} ${fmt(-cubeSize / 2)} ${fmt(-flatCut)}`);

    // Line 3: Intersection creates knob body
    lines.push(`I 0 2`);

    let baseIdx = 3;

    if (p.hasInsert) {
      // Add threaded insert hole from bottom
      const holeIdx = lines.length;
      lines.push(`Y ${fmt(p.insertDiameter / 2)} ${fmt(p.insertDepth)}`);

      const transIdx = lines.length;
      // Position hole coming from bottom (at -flatCut)
      lines.push(`T ${holeIdx} 0 0 ${fmt(-flatCut)}`);

      lines.push(`D ${baseIdx} ${transIdx}`);
    }
  }

  private generateHandle(lines: string[], p: BallParams): void {
    // Ball with cylindrical stem for handle knob
    // Line 0: Sphere
    lines.push(`S ${fmt(p.radius)}`);

    // Line 1: Stem cylinder
    lines.push(`Y ${fmt(p.stemDiameter / 2)} ${fmt(p.stemLength)}`);

    // Line 2: Position stem below sphere
    lines.push(`T 1 0 0 ${fmt(-p.radius - p.stemLength)}`);

    // Line 3: Union ball and stem
    lines.push(`U 0 2`);
  }

  private computeComplexity(p: BallParams): number {
    switch (p.ballType) {
      case "sphere":
        return 1;
      case "dome":
        return 2;
      case "drilled":
        return 2;
      case "knob":
        return p.hasInsert ? 3 : 2;
      case "handle":
        return 2;
      default:
        return 1;
    }
  }
}
