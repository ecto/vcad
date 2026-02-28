/**
 * Spacer generator - rings, tubes, and standoffs.
 *
 * Generates simple cylindrical spacers:
 * - Solid cylinders (standoffs)
 * - Hollow tubes (through-hole spacers)
 * - Flanged spacers
 */

import type {
  PartGenerator,
  PartParams,
  ParamDef,
  GeneratedPart,
} from "./types.js";
import { randInt, randFloat, randChoice, randBool, fmt } from "./utils.js";

export type SpacerType = "solid" | "hollow" | "flanged";

export interface SpacerParams extends PartParams {
  outerDiameter: number;
  height: number;
  spacerType: SpacerType;
  innerDiameter: number;
  flangeOuterDiameter: number;
  flangeThickness: number;
  flangePosition: "bottom" | "top" | "both";
}

export class SpacerGenerator implements PartGenerator {
  readonly family = "spacer";
  readonly description = "Rings, tubes, and standoff spacers";

  paramDefs(): Record<string, ParamDef> {
    return {
      outerDiameter: {
        type: "number",
        range: { min: 5, max: 30, step: 1 },
        description: "Outer diameter (mm)",
      },
      height: {
        type: "number",
        range: { min: 3, max: 50, step: 1 },
        description: "Spacer height (mm)",
      },
      spacerType: {
        type: "choice",
        choices: ["solid", "hollow", "flanged"],
        description: "Spacer type",
      },
      innerDiameter: {
        type: "number",
        range: { min: 2, max: 15, step: 0.5 },
        description: "Inner hole diameter (mm)",
      },
      flangeOuterDiameter: {
        type: "number",
        range: { min: 10, max: 50, step: 2 },
        description: "Flange outer diameter (mm)",
      },
      flangeThickness: {
        type: "number",
        range: { min: 1, max: 5, step: 0.5 },
        description: "Flange thickness (mm)",
      },
      flangePosition: {
        type: "choice",
        choices: ["bottom", "top", "both"],
        description: "Flange position",
      },
    };
  }

  randomParams(): SpacerParams {
    const defs = this.paramDefs();
    const outerDiameter = randInt(defs.outerDiameter.range!);
    return {
      outerDiameter,
      height: randInt(defs.height.range!),
      spacerType: randChoice(defs.spacerType.choices!) as SpacerType,
      innerDiameter: randFloat(
        { min: defs.innerDiameter.range!.min, max: outerDiameter * 0.6 },
        1,
      ),
      flangeOuterDiameter: randInt({
        min: outerDiameter + 4,
        max: Math.min(defs.flangeOuterDiameter.range!.max, outerDiameter * 2),
      }),
      flangeThickness: randFloat(defs.flangeThickness.range!, 1),
      flangePosition: randChoice(defs.flangePosition.choices!) as "bottom" | "top" | "both",
    };
  }

  generate(params?: Partial<SpacerParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as SpacerParams;

    // Ensure constraints
    p.innerDiameter = Math.min(p.innerDiameter, p.outerDiameter * 0.8);
    p.flangeOuterDiameter = Math.max(p.flangeOuterDiameter, p.outerDiameter + 2);

    const lines: string[] = [];

    switch (p.spacerType) {
      case "solid":
        // Simple solid cylinder
        lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.height)}`);
        break;

      case "hollow":
        // Outer cylinder minus inner cylinder
        lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.height)}`);
        lines.push(`Y ${fmt(p.innerDiameter / 2)} ${fmt(p.height + 2)}`);
        lines.push(`T 1 0 0 -1`); // Center inner cylinder
        lines.push(`D 0 2`);
        break;

      case "flanged":
        this.generateFlangedSpacer(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateFlangedSpacer(lines: string[], p: SpacerParams): void {
    // Main body cylinder
    lines.push(`Y ${fmt(p.outerDiameter / 2)} ${fmt(p.height)}`);
    let baseIdx = 0;

    // Bottom flange
    if (p.flangePosition === "bottom" || p.flangePosition === "both") {
      const flangeIdx = lines.length;
      lines.push(`Y ${fmt(p.flangeOuterDiameter / 2)} ${fmt(p.flangeThickness)}`);
      const unionIdx = lines.length;
      lines.push(`U ${baseIdx} ${flangeIdx}`);
      baseIdx = unionIdx;
    }

    // Top flange
    if (p.flangePosition === "top" || p.flangePosition === "both") {
      const flangeIdx = lines.length;
      lines.push(`Y ${fmt(p.flangeOuterDiameter / 2)} ${fmt(p.flangeThickness)}`);
      const transIdx = lines.length;
      lines.push(`T ${flangeIdx} 0 0 ${fmt(p.height - p.flangeThickness)}`);
      const unionIdx = lines.length;
      lines.push(`U ${baseIdx} ${transIdx}`);
      baseIdx = unionIdx;
    }

    // Center hole
    const holeIdx = lines.length;
    lines.push(`Y ${fmt(p.innerDiameter / 2)} ${fmt(p.height + 4)}`);
    const holeTransIdx = lines.length;
    lines.push(`T ${holeIdx} 0 0 -2`);
    lines.push(`D ${baseIdx} ${holeTransIdx}`);
  }

  private computeComplexity(p: SpacerParams): number {
    if (p.spacerType === "solid") return 1;
    if (p.spacerType === "hollow") return 1;
    // Flanged with both flanges
    if (p.flangePosition === "both") return 2;
    return 2;
  }
}
