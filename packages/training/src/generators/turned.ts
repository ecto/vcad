/**
 * Turned generator - parts using SK (Sketch) + V (Revolve) operations.
 *
 * Generates:
 * - Bottle/vase shape
 * - Pulley/wheel
 * - Knurled knob
 * - Stepped shaft (lathe-turned)
 * - Bowl
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

export type TurnedType = "bottle" | "pulley" | "knob" | "steppedShaft" | "bowl";

export interface TurnedParams extends PartParams {
  turnedType: TurnedType;
  /** Overall height */
  height: number;
  /** Maximum radius */
  maxRadius: number;
  /** Revolve angle (usually 360) */
  angleDeg: number;
  /** For bottle: neck radius */
  neckRadius: number;
  /** For bottle: neck height */
  neckHeight: number;
  /** For pulley: groove depth */
  grooveDepth: number;
  /** For pulley: groove width */
  grooveWidth: number;
  /** For stepped shaft: number of steps */
  steps: number;
  /** For bowl: wall thickness */
  wallThickness: number;
  /** For bowl: rim radius */
  rimRadius: number;
}

export class TurnedGenerator implements PartGenerator {
  readonly family = "turned";
  readonly description = "Parts using SK (Sketch) + V (Revolve) operations";

  paramDefs(): Record<string, ParamDef> {
    return {
      turnedType: {
        type: "choice",
        choices: ["bottle", "pulley", "knob", "steppedShaft", "bowl"],
        description: "Type of revolved part",
      },
      height: {
        type: "number",
        range: { min: 20, max: 100, step: 5 },
        description: "Overall height (mm)",
      },
      maxRadius: {
        type: "number",
        range: { min: 10, max: 50, step: 2 },
        description: "Maximum radius (mm)",
      },
      angleDeg: {
        type: "number",
        range: { min: 180, max: 360, step: 30 },
        description: "Revolve angle (degrees)",
      },
      neckRadius: {
        type: "number",
        range: { min: 3, max: 15, step: 1 },
        description: "Bottle neck radius (mm)",
      },
      neckHeight: {
        type: "number",
        range: { min: 5, max: 25, step: 2 },
        description: "Bottle neck height (mm)",
      },
      grooveDepth: {
        type: "number",
        range: { min: 2, max: 10, step: 1 },
        description: "Pulley groove depth (mm)",
      },
      grooveWidth: {
        type: "number",
        range: { min: 3, max: 15, step: 1 },
        description: "Pulley groove width (mm)",
      },
      steps: {
        type: "number",
        range: { min: 2, max: 4, step: 1 },
        description: "Number of shaft steps",
      },
      wallThickness: {
        type: "number",
        range: { min: 2, max: 6, step: 0.5 },
        description: "Bowl wall thickness (mm)",
      },
      rimRadius: {
        type: "number",
        range: { min: 15, max: 50, step: 2 },
        description: "Bowl rim radius (mm)",
      },
    };
  }

  randomParams(): TurnedParams {
    const defs = this.paramDefs();
    const maxRadius = randInt(defs.maxRadius.range!);

    return {
      turnedType: randChoice(defs.turnedType.choices!) as TurnedType,
      height: randInt(defs.height.range!),
      maxRadius,
      angleDeg: randChoice([270, 360]),
      neckRadius: randInt({
        min: defs.neckRadius.range!.min,
        max: Math.min(defs.neckRadius.range!.max, maxRadius * 0.5),
      }),
      neckHeight: randInt(defs.neckHeight.range!),
      grooveDepth: randInt(defs.grooveDepth.range!),
      grooveWidth: randInt(defs.grooveWidth.range!),
      steps: randInt(defs.steps.range!),
      wallThickness: randFloat(defs.wallThickness.range!, 1),
      rimRadius: randInt({
        min: defs.rimRadius.range!.min,
        max: Math.min(defs.rimRadius.range!.max, maxRadius),
      }),
    };
  }

  generate(params?: Partial<TurnedParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as TurnedParams;

    // Ensure constraints
    p.neckRadius = Math.min(p.neckRadius, p.maxRadius * 0.6);
    p.grooveDepth = Math.min(p.grooveDepth, p.maxRadius * 0.4);

    const lines: string[] = [];

    switch (p.turnedType) {
      case "bottle":
        this.generateBottle(lines, p);
        break;
      case "pulley":
        this.generatePulley(lines, p);
        break;
      case "knob":
        this.generateKnob(lines, p);
        break;
      case "steppedShaft":
        this.generateSteppedShaft(lines, p);
        break;
      case "bowl":
        this.generateBowl(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateBottle(lines: string[], p: TurnedParams): void {
    // Bottle: body with narrowing neck
    // Profile revolved around Y axis (which becomes Z after revolve)
    // Sketch on XZ plane (sketch Y = world Z)

    const bodyHeight = p.height - p.neckHeight;
    const r = p.maxRadius;
    const nr = p.neckRadius;
    const nh = p.neckHeight;

    // Sketch in XY plane, revolve around Y axis (Y becomes rotation axis)
    // Profile is on right side of Y axis (positive X)
    lines.push(`SK 0 0 0  1 0 0  0 0 1`);

    // Draw bottle profile (right side only, will be revolved)
    // Start at bottom center, go up right side
    lines.push(`L 0 0 ${fmt(r)} 0`);  // Bottom to outer edge
    lines.push(`L ${fmt(r)} 0 ${fmt(r)} ${fmt(bodyHeight * 0.7)}`);  // Straight body
    lines.push(`L ${fmt(r)} ${fmt(bodyHeight * 0.7)} ${fmt(nr)} ${fmt(bodyHeight)}`);  // Taper to neck
    lines.push(`L ${fmt(nr)} ${fmt(bodyHeight)} ${fmt(nr)} ${fmt(p.height)}`);  // Neck
    lines.push(`L ${fmt(nr)} ${fmt(p.height)} 0 ${fmt(p.height)}`);  // Top to center
    lines.push(`L 0 ${fmt(p.height)} 0 0`);  // Back down axis (close)
    lines.push(`END`);

    // Revolve around Y axis (0,0,0 origin, 0,1,0 direction for Y axis)
    // But sketch is on XZ, so axis should be Z direction: 0,0,1
    lines.push(`V 0 0 0 0 0 0 1 ${fmt(p.angleDeg)}`);
  }

  private generatePulley(lines: string[], p: TurnedParams): void {
    // Pulley: disc with V-groove around circumference
    const r = p.maxRadius;
    const gr = r - p.grooveDepth; // Groove bottom radius
    const gw = p.grooveWidth;
    const h = Math.max(gw * 1.5, p.height * 0.3);
    const hubH = p.height - h;

    lines.push(`SK 0 0 0  1 0 0  0 0 1`);

    // Pulley profile with V-groove
    // Start at center bottom
    lines.push(`L 0 0 ${fmt(r * 0.3)} 0`);  // Hub inner
    lines.push(`L ${fmt(r * 0.3)} 0 ${fmt(r * 0.3)} ${fmt(hubH)}`);  // Hub wall
    lines.push(`L ${fmt(r * 0.3)} ${fmt(hubH)} ${fmt(r)} ${fmt(hubH)}`);  // Hub top
    lines.push(`L ${fmt(r)} ${fmt(hubH)} ${fmt(r)} ${fmt(hubH + (h - gw) / 2)}`);  // Pulley outer before groove
    lines.push(`L ${fmt(r)} ${fmt(hubH + (h - gw) / 2)} ${fmt(gr)} ${fmt(hubH + h / 2)}`);  // Groove down
    lines.push(`L ${fmt(gr)} ${fmt(hubH + h / 2)} ${fmt(r)} ${fmt(hubH + (h + gw) / 2)}`);  // Groove up
    lines.push(`L ${fmt(r)} ${fmt(hubH + (h + gw) / 2)} ${fmt(r)} ${fmt(hubH + h)}`);  // Pulley outer after groove
    lines.push(`L ${fmt(r)} ${fmt(hubH + h)} 0 ${fmt(hubH + h)}`);  // Top to center
    lines.push(`L 0 ${fmt(hubH + h)} 0 0`);  // Close
    lines.push(`END`);

    lines.push(`V 0 0 0 0 0 0 1 360`);
  }

  private generateKnob(lines: string[], p: TurnedParams): void {
    // Rounded knob with finger grip profile
    const r = p.maxRadius;
    const h = p.height;

    lines.push(`SK 0 0 0  1 0 0  0 0 1`);

    // Knob profile: bulging shape
    const midR = r * 0.85;
    const topR = r * 0.6;

    lines.push(`L 0 0 ${fmt(r * 0.4)} 0`);  // Base to stem
    lines.push(`L ${fmt(r * 0.4)} 0 ${fmt(r * 0.4)} ${fmt(h * 0.15)}`);  // Stem
    lines.push(`L ${fmt(r * 0.4)} ${fmt(h * 0.15)} ${fmt(midR)} ${fmt(h * 0.3)}`);  // Flare out
    lines.push(`L ${fmt(midR)} ${fmt(h * 0.3)} ${fmt(r)} ${fmt(h * 0.5)}`);  // Max radius
    lines.push(`L ${fmt(r)} ${fmt(h * 0.5)} ${fmt(midR)} ${fmt(h * 0.7)}`);  // Curve in
    lines.push(`L ${fmt(midR)} ${fmt(h * 0.7)} ${fmt(topR)} ${fmt(h * 0.9)}`);  // Near top
    lines.push(`L ${fmt(topR)} ${fmt(h * 0.9)} ${fmt(topR * 0.5)} ${fmt(h)}`);  // Top edge
    lines.push(`L ${fmt(topR * 0.5)} ${fmt(h)} 0 ${fmt(h)}`);  // Top center
    lines.push(`L 0 ${fmt(h)} 0 0`);  // Close
    lines.push(`END`);

    lines.push(`V 0 0 0 0 0 0 1 360`);
  }

  private generateSteppedShaft(lines: string[], p: TurnedParams): void {
    // Stepped shaft: multiple diameter sections (like lathe-turned)
    // Ensure steps is an integer >= 2 (in case of conversation modifications)
    const steps = Math.max(2, Math.round(p.steps || 3));
    const h = p.height;
    const stepH = h / steps;

    lines.push(`SK 0 0 0  1 0 0  0 0 1`);

    // Generate decreasing radii for each step
    const radii: number[] = [];
    for (let i = 0; i < steps; i++) {
      radii.push(p.maxRadius * (1 - i * 0.2));
    }

    // Draw stepped profile
    let currentY = 0;
    lines.push(`L 0 0 ${fmt(radii[0])} 0`);  // Bottom to first radius

    for (let i = 0; i < steps; i++) {
      const r = radii[i];
      const nextY = currentY + stepH;

      // Vertical wall
      lines.push(`L ${fmt(r)} ${fmt(currentY)} ${fmt(r)} ${fmt(nextY)}`);

      // Horizontal step to next diameter (if not last)
      if (i < steps - 1) {
        const nextR = radii[i + 1];
        lines.push(`L ${fmt(r)} ${fmt(nextY)} ${fmt(nextR)} ${fmt(nextY)}`);
      }

      currentY = nextY;
    }

    // Close the profile
    lines.push(`L ${fmt(radii[steps - 1])} ${fmt(h)} 0 ${fmt(h)}`);  // Top to center
    lines.push(`L 0 ${fmt(h)} 0 0`);  // Close
    lines.push(`END`);

    lines.push(`V 0 0 0 0 0 0 1 360`);
  }

  private generateBowl(lines: string[], p: TurnedParams): void {
    // Bowl: curved shell shape
    const r = p.rimRadius;
    const h = p.height;
    const t = p.wallThickness;

    lines.push(`SK 0 0 0  1 0 0  0 0 1`);

    // Bowl outer profile (curved)
    const midR = r * 0.9;
    const baseR = r * 0.4;

    // Outer curve
    lines.push(`L 0 0 ${fmt(baseR)} 0`);  // Center to base
    lines.push(`L ${fmt(baseR)} 0 ${fmt(midR)} ${fmt(h * 0.3)}`);  // Curve up
    lines.push(`L ${fmt(midR)} ${fmt(h * 0.3)} ${fmt(r)} ${fmt(h * 0.7)}`);  // Continue curve
    lines.push(`L ${fmt(r)} ${fmt(h * 0.7)} ${fmt(r)} ${fmt(h)}`);  // Rim outer

    // Inner curve (wall thickness offset)
    const innerR = r - t;
    const innerMidR = midR - t;
    const innerBaseR = Math.max(baseR - t, t);

    lines.push(`L ${fmt(r)} ${fmt(h)} ${fmt(innerR)} ${fmt(h)}`);  // Rim top
    lines.push(`L ${fmt(innerR)} ${fmt(h)} ${fmt(innerR)} ${fmt(h * 0.7)}`);  // Inner rim
    lines.push(`L ${fmt(innerR)} ${fmt(h * 0.7)} ${fmt(innerMidR)} ${fmt(h * 0.3)}`);  // Inner curve
    lines.push(`L ${fmt(innerMidR)} ${fmt(h * 0.3)} ${fmt(innerBaseR)} ${fmt(t)}`);  // Inner base approach
    lines.push(`L ${fmt(innerBaseR)} ${fmt(t)} 0 ${fmt(t)}`);  // Inner bottom to center
    lines.push(`L 0 ${fmt(t)} 0 0`);  // Close
    lines.push(`END`);

    lines.push(`V 0 0 0 0 0 0 1 360`);
  }

  private computeComplexity(p: TurnedParams): number {
    switch (p.turnedType) {
      case "bottle":
        return 3;
      case "pulley":
        return 3;
      case "knob":
        return 3;
      case "steppedShaft":
        return Math.min(2 + p.steps, 5);
      case "bowl":
        return 3;
      default:
        return 2;
    }
  }
}
