/**
 * Hollow generator - parts using the SH (Shell) operation.
 *
 * Generates:
 * - Hollow box/enclosure
 * - Tube (shelled cylinder)
 * - Cup/container
 * - Housing with uniform wall
 * - Dome shell
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

export type HollowType = "box" | "tube" | "cup" | "housing" | "domeShell";

export interface HollowParams extends PartParams {
  hollowType: HollowType;
  /** Outer width/diameter */
  outerWidth: number;
  /** Outer depth */
  outerDepth: number;
  /** Outer height */
  outerHeight: number;
  /** Wall thickness */
  wallThickness: number;
  /** For tube: outer diameter */
  outerDiameter: number;
  /** For cup: has handle */
  hasHandle: boolean;
  /** Handle width */
  handleWidth: number;
  /** Handle height */
  handleHeight: number;
  /** For housing: has mounting tabs */
  hasTabs: boolean;
  /** Tab width */
  tabWidth: number;
  /** Tab hole diameter */
  tabHoleDiameter: number;
}

export class HollowGenerator implements PartGenerator {
  readonly family = "hollow";
  readonly description = "Parts using SH (Shell) operation";

  paramDefs(): Record<string, ParamDef> {
    return {
      hollowType: {
        type: "choice",
        choices: ["box", "tube", "cup", "housing", "domeShell"],
        description: "Type of shelled part",
      },
      outerWidth: {
        type: "number",
        range: { min: 20, max: 100, step: 5 },
        description: "Outer width (mm)",
      },
      outerDepth: {
        type: "number",
        range: { min: 20, max: 100, step: 5 },
        description: "Outer depth (mm)",
      },
      outerHeight: {
        type: "number",
        range: { min: 15, max: 80, step: 5 },
        description: "Outer height (mm)",
      },
      wallThickness: {
        type: "number",
        range: { min: 1, max: 5, step: 0.5 },
        description: "Wall thickness (mm)",
      },
      outerDiameter: {
        type: "number",
        range: { min: 15, max: 80, step: 5 },
        description: "Outer diameter for tube (mm)",
      },
      hasHandle: {
        type: "boolean",
        description: "Cup has handle",
      },
      handleWidth: {
        type: "number",
        range: { min: 5, max: 15, step: 1 },
        description: "Handle width (mm)",
      },
      handleHeight: {
        type: "number",
        range: { min: 15, max: 40, step: 5 },
        description: "Handle height (mm)",
      },
      hasTabs: {
        type: "boolean",
        description: "Housing has mounting tabs",
      },
      tabWidth: {
        type: "number",
        range: { min: 8, max: 20, step: 2 },
        description: "Mounting tab width (mm)",
      },
      tabHoleDiameter: {
        type: "number",
        range: { min: 3, max: 8, step: 0.5 },
        description: "Tab hole diameter (mm)",
      },
    };
  }

  randomParams(): HollowParams {
    const defs = this.paramDefs();
    return {
      hollowType: randChoice(defs.hollowType.choices!) as HollowType,
      outerWidth: randInt(defs.outerWidth.range!),
      outerDepth: randInt(defs.outerDepth.range!),
      outerHeight: randInt(defs.outerHeight.range!),
      wallThickness: randFloat(defs.wallThickness.range!, 1),
      outerDiameter: randInt(defs.outerDiameter.range!),
      hasHandle: randBool(0.4),
      handleWidth: randInt(defs.handleWidth.range!),
      handleHeight: randInt(defs.handleHeight.range!),
      hasTabs: randBool(0.5),
      tabWidth: randInt(defs.tabWidth.range!),
      tabHoleDiameter: randFloat(defs.tabHoleDiameter.range!, 1),
    };
  }

  generate(params?: Partial<HollowParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as HollowParams;

    // Ensure constraints
    const minDim = Math.min(p.outerWidth, p.outerDepth, p.outerHeight, p.outerDiameter);
    p.wallThickness = Math.min(p.wallThickness, minDim / 4);

    const lines: string[] = [];

    switch (p.hollowType) {
      case "box":
        this.generateBox(lines, p);
        break;
      case "tube":
        this.generateTube(lines, p);
        break;
      case "cup":
        this.generateCup(lines, p);
        break;
      case "housing":
        this.generateHousing(lines, p);
        break;
      case "domeShell":
        this.generateDomeShell(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateBox(lines: string[], p: HollowParams): void {
    // Simple hollow box using shell operation
    // Line 0: Solid cube
    lines.push(`C ${fmt(p.outerWidth)} ${fmt(p.outerDepth)} ${fmt(p.outerHeight)}`);

    // Line 1: Shell it (removes top face, hollows out interior)
    lines.push(`SH 0 ${fmt(p.wallThickness)}`);
  }

  private generateTube(lines: string[], p: HollowParams): void {
    // Cylindrical tube using shell
    // Line 0: Solid cylinder
    lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.outerHeight)}`);

    // Line 1: Shell it
    lines.push(`SH 0 ${fmt(p.wallThickness)}`);
  }

  private generateCup(lines: string[], p: HollowParams): void {
    // Cup: shelled cylinder with optional handle
    // Line 0: Solid cylinder for cup body
    lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.outerHeight)}`);

    // Line 1: Shell to create cup
    lines.push(`SH 0 ${fmt(p.wallThickness)}`);

    if (p.hasHandle) {
      // Create a simple handle by adding a torus-like structure
      // Approximated with two cylinders forming a loop
      const handleThickness = p.handleWidth / 3;

      // Line 2: Handle outer arc (approximated as vertical cylinder segment)
      // We'll use a simple rectangular handle for training simplicity
      lines.push(`C ${fmt(p.handleWidth)} ${fmt(handleThickness)} ${fmt(p.handleHeight)}`);

      // Line 3: Position handle at cup edge
      lines.push(`T 2 ${fmt(p.outerDiameter / 2)} ${fmt(-handleThickness / 2)} ${fmt((p.outerHeight - p.handleHeight) / 2)}`);

      // Line 4: Union handle with cup
      lines.push(`U 1 3`);
    }
  }

  private generateHousing(lines: string[], p: HollowParams): void {
    // Housing: shelled box with optional mounting tabs
    // Line 0: Solid box
    lines.push(`C ${fmt(p.outerWidth)} ${fmt(p.outerDepth)} ${fmt(p.outerHeight)}`);

    // Line 1: Shell it
    lines.push(`SH 0 ${fmt(p.wallThickness)}`);

    if (p.hasTabs) {
      // Add mounting tabs at corners
      const tabThickness = p.wallThickness * 1.5;
      const tabLength = p.tabWidth;

      // Line 2: Single tab
      lines.push(`C ${fmt(tabLength)} ${fmt(tabLength)} ${fmt(tabThickness)}`);

      // Line 3: Hole in tab
      lines.push(`Y ${fmt(p.tabHoleDiameter / 2)} ${fmt(tabThickness + 2)}`);

      // Line 4: Position hole in tab center
      lines.push(`T 3 ${fmt(tabLength / 2)} ${fmt(tabLength / 2)} -1`);

      // Line 5: Subtract hole from tab
      lines.push(`D 2 4`);

      // Line 6: Position tab at corner (negative X, negative Y)
      lines.push(`T 5 ${fmt(-tabLength)} ${fmt(-tabLength)} 0`);

      // Line 7: Union with housing
      lines.push(`U 1 6`);

      // Add second tab at opposite corner
      // Line 8: Another tab with hole
      lines.push(`C ${fmt(tabLength)} ${fmt(tabLength)} ${fmt(tabThickness)}`);

      // Line 9: Hole
      lines.push(`Y ${fmt(p.tabHoleDiameter / 2)} ${fmt(tabThickness + 2)}`);

      // Line 10: Position hole
      lines.push(`T 9 ${fmt(tabLength / 2)} ${fmt(tabLength / 2)} -1`);

      // Line 11: Subtract
      lines.push(`D 8 10`);

      // Line 12: Position at opposite corner
      lines.push(`T 11 ${fmt(p.outerWidth)} ${fmt(p.outerDepth)} 0`);

      // Line 13: Union all
      lines.push(`U 7 12`);
    }
  }

  private generateDomeShell(lines: string[], p: HollowParams): void {
    // Dome shell: hemisphere with shell
    const radius = p.outerDiameter / 2;

    // Line 0: Sphere
    lines.push(`S ${fmt(radius)}`);

    // Line 1: Cutting cube to make hemisphere (keep top half)
    const cubeSize = radius * 2.2;
    lines.push(`C ${fmt(cubeSize)} ${fmt(cubeSize)} ${fmt(radius + 1)}`);

    // Line 2: Position cube to keep top hemisphere
    lines.push(`T 1 ${fmt(-cubeSize / 2)} ${fmt(-cubeSize / 2)} 0`);

    // Line 3: Intersection for hemisphere
    lines.push(`I 0 2`);

    // Line 4: Shell the hemisphere
    lines.push(`SH 3 ${fmt(p.wallThickness)}`);
  }

  private computeComplexity(p: HollowParams): number {
    let complexity = 2; // Base for shell operation

    if (p.hollowType === "cup" && p.hasHandle) complexity++;
    if (p.hollowType === "housing" && p.hasTabs) complexity += 2;
    if (p.hollowType === "domeShell") complexity++;

    return Math.min(complexity, 4);
  }
}
