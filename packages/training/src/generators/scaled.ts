/**
 * Scaled generator - parts using the X (Scale) operation for non-uniform scaling.
 *
 * Generates:
 * - Elliptical disc (scaled cylinder)
 * - Stretched block (non-uniformly scaled cube)
 * - Ellipsoid (scaled sphere)
 * - Tapered block (scaled + boolean combination)
 * - Oval tube (scaled hollow cylinder)
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

export type ScaledType = "ellipse" | "stretched" | "ellipsoid" | "tapered" | "oval";

export interface ScaledParams extends PartParams {
  scaledType: ScaledType;
  /** Base dimension */
  baseSize: number;
  /** Height/length */
  height: number;
  /** Scale factor X */
  scaleX: number;
  /** Scale factor Y */
  scaleY: number;
  /** Scale factor Z */
  scaleZ: number;
  /** For oval tube: wall thickness */
  wallThickness: number;
  /** For tapered: taper ratio */
  taperRatio: number;
}

export class ScaledGenerator implements PartGenerator {
  readonly family = "scaled";
  readonly description = "Parts using X (Scale) operation for non-uniform scaling";

  paramDefs(): Record<string, ParamDef> {
    return {
      scaledType: {
        type: "choice",
        choices: ["ellipse", "stretched", "ellipsoid", "tapered", "oval"],
        description: "Type of scaled part",
      },
      baseSize: {
        type: "number",
        range: { min: 10, max: 50, step: 2 },
        description: "Base dimension before scaling (mm)",
      },
      height: {
        type: "number",
        range: { min: 5, max: 60, step: 2 },
        description: "Height/length (mm)",
      },
      scaleX: {
        type: "number",
        range: { min: 0.3, max: 3, step: 0.1 },
        description: "Scale factor in X direction",
      },
      scaleY: {
        type: "number",
        range: { min: 0.3, max: 3, step: 0.1 },
        description: "Scale factor in Y direction",
      },
      scaleZ: {
        type: "number",
        range: { min: 0.3, max: 3, step: 0.1 },
        description: "Scale factor in Z direction",
      },
      wallThickness: {
        type: "number",
        range: { min: 1, max: 5, step: 0.5 },
        description: "Wall thickness for hollow types (mm)",
      },
      taperRatio: {
        type: "number",
        range: { min: 0.3, max: 0.9, step: 0.1 },
        description: "Taper ratio for tapered block",
      },
    };
  }

  randomParams(): ScaledParams {
    const defs = this.paramDefs();
    const scaledType = randChoice(defs.scaledType.choices!) as ScaledType;

    // Generate scale factors based on type
    let scaleX = 1;
    let scaleY = 1;
    let scaleZ = 1;

    switch (scaledType) {
      case "ellipse":
        // Elliptical disc: scale X differently from Y, Z is 1
        scaleX = randFloat({ min: 0.5, max: 2, step: 0.1 }, 1);
        scaleY = 1;
        scaleZ = 1;
        break;
      case "stretched":
        // Stretched block: one axis stretched
        scaleX = randFloat({ min: 1, max: 2.5, step: 0.1 }, 1);
        scaleY = randFloat({ min: 0.5, max: 1, step: 0.1 }, 1);
        scaleZ = 1;
        break;
      case "ellipsoid":
        // Ellipsoid: all three axes different
        scaleX = randFloat({ min: 0.5, max: 2, step: 0.1 }, 1);
        scaleY = randFloat({ min: 0.5, max: 2, step: 0.1 }, 1);
        scaleZ = randFloat({ min: 0.5, max: 2, step: 0.1 }, 1);
        break;
      case "tapered":
        // Tapered uses scale differently - see generation
        scaleX = randFloat({ min: 0.4, max: 0.8, step: 0.1 }, 1);
        scaleY = randFloat({ min: 0.4, max: 0.8, step: 0.1 }, 1);
        scaleZ = 1;
        break;
      case "oval":
        // Oval tube: elliptical cross-section
        scaleX = randFloat({ min: 0.5, max: 2, step: 0.1 }, 1);
        scaleY = 1;
        scaleZ = 1;
        break;
    }

    return {
      scaledType,
      baseSize: randInt(defs.baseSize.range!),
      height: randInt(defs.height.range!),
      scaleX,
      scaleY,
      scaleZ,
      wallThickness: randFloat(defs.wallThickness.range!, 1),
      taperRatio: randFloat(defs.taperRatio.range!, 1),
    };
  }

  generate(params?: Partial<ScaledParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as ScaledParams;

    const lines: string[] = [];

    switch (p.scaledType) {
      case "ellipse":
        this.generateEllipse(lines, p);
        break;
      case "stretched":
        this.generateStretched(lines, p);
        break;
      case "ellipsoid":
        this.generateEllipsoid(lines, p);
        break;
      case "tapered":
        this.generateTapered(lines, p);
        break;
      case "oval":
        this.generateOval(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateEllipse(lines: string[], p: ScaledParams): void {
    // Elliptical disc: cylinder scaled in X to create ellipse
    // Line 0: Cylinder (base disc)
    lines.push(`Y ${fmt(p.baseSize / 2)} ${fmt(p.height)}`);

    // Line 1: Scale to create elliptical cross-section
    lines.push(`X 0 ${fmt(p.scaleX)} 1 1`);
  }

  private generateStretched(lines: string[], p: ScaledParams): void {
    // Stretched block: cube with non-uniform scaling
    // Line 0: Cube
    lines.push(`C ${fmt(p.baseSize)} ${fmt(p.baseSize)} ${fmt(p.height)}`);

    // Line 1: Scale to stretch
    lines.push(`X 0 ${fmt(p.scaleX)} ${fmt(p.scaleY)} 1`);
  }

  private generateEllipsoid(lines: string[], p: ScaledParams): void {
    // Ellipsoid: sphere scaled differently in each axis
    // Line 0: Sphere
    lines.push(`S ${fmt(p.baseSize)}`);

    // Line 1: Scale to create ellipsoid
    lines.push(`X 0 ${fmt(p.scaleX)} ${fmt(p.scaleY)} ${fmt(p.scaleZ)}`);
  }

  private generateTapered(lines: string[], p: ScaledParams): void {
    // Tapered block: approximated by combining a scaled block
    // This creates a block that's narrower at one end
    // For true taper we'd need a cone, but this shows Scale usage

    // Line 0: Full-size block (base)
    lines.push(`C ${fmt(p.baseSize)} ${fmt(p.baseSize)} ${fmt(p.height / 2)}`);

    // Line 1: Smaller block for top
    const topSize = p.baseSize * p.taperRatio;
    lines.push(`C ${fmt(topSize)} ${fmt(topSize)} ${fmt(p.height / 2)}`);

    // Line 2: Scale the top block's position offset
    // Center the top block
    const offset = (p.baseSize - topSize) / 2;
    lines.push(`T 1 ${fmt(offset)} ${fmt(offset)} ${fmt(p.height / 2)}`);

    // Line 3: Union the two parts
    lines.push(`U 0 2`);
  }

  private generateOval(lines: string[], p: ScaledParams): void {
    // Oval tube: scaled hollow cylinder
    // Create elliptical cross-section tube

    // Line 0: Outer cylinder
    lines.push(`Y ${fmt(p.baseSize / 2)} ${fmt(p.height)}`);

    // Line 1: Inner cylinder
    const innerRadius = p.baseSize / 2 - p.wallThickness;
    lines.push(`Y ${fmt(innerRadius)} ${fmt(p.height + 2)}`);

    // Line 2: Position inner cylinder
    lines.push(`T 1 0 0 -1`);

    // Line 3: Subtract to make hollow
    lines.push(`D 0 2`);

    // Line 4: Scale to create oval cross-section
    lines.push(`X 3 ${fmt(p.scaleX)} 1 1`);
  }

  private computeComplexity(p: ScaledParams): number {
    switch (p.scaledType) {
      case "ellipse":
      case "stretched":
      case "ellipsoid":
        return 1;
      case "tapered":
      case "oval":
        return 2;
      default:
        return 1;
    }
  }
}
