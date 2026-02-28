/**
 * Funnel generator - conical parts using the K (Cone) primitive.
 *
 * Generates:
 * - Simple cone/funnel
 * - Truncated cone (frustum)
 * - Conical adapter (cone + cylinder union)
 * - Countersunk pocket (cone + cyl difference)
 * - Hopper (hollow cone)
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

export type FunnelType = "cone" | "frustum" | "adapter" | "countersink" | "hopper";

export interface FunnelParams extends PartParams {
  bottomRadius: number;
  topRadius: number;
  height: number;
  funnelType: FunnelType;
  /** For adapter: extension cylinder radius */
  extensionRadius: number;
  /** For adapter: extension cylinder height */
  extensionHeight: number;
  /** For hopper: wall thickness */
  wallThickness: number;
  /** For countersink: base plate dimensions */
  plateWidth: number;
  plateDepth: number;
  plateThickness: number;
}

export class FunnelGenerator implements PartGenerator {
  readonly family = "funnel";
  readonly description = "Conical parts using K (Cone) primitive";

  paramDefs(): Record<string, ParamDef> {
    return {
      bottomRadius: {
        type: "number",
        range: { min: 8, max: 50, step: 1 },
        description: "Cone bottom radius (mm)",
      },
      topRadius: {
        type: "number",
        range: { min: 0, max: 40, step: 1 },
        description: "Cone top radius (mm) - 0 for pointed cone",
      },
      height: {
        type: "number",
        range: { min: 10, max: 80, step: 2 },
        description: "Cone height (mm)",
      },
      funnelType: {
        type: "choice",
        choices: ["cone", "frustum", "adapter", "countersink", "hopper"],
        description: "Type of conical part",
      },
      extensionRadius: {
        type: "number",
        range: { min: 5, max: 30, step: 1 },
        description: "Extension cylinder radius (mm)",
      },
      extensionHeight: {
        type: "number",
        range: { min: 5, max: 40, step: 2 },
        description: "Extension cylinder height (mm)",
      },
      wallThickness: {
        type: "number",
        range: { min: 1.5, max: 5, step: 0.5 },
        description: "Wall thickness for hopper (mm)",
      },
      plateWidth: {
        type: "number",
        range: { min: 30, max: 100, step: 5 },
        description: "Base plate width (mm)",
      },
      plateDepth: {
        type: "number",
        range: { min: 30, max: 100, step: 5 },
        description: "Base plate depth (mm)",
      },
      plateThickness: {
        type: "number",
        range: { min: 3, max: 15, step: 1 },
        description: "Base plate thickness (mm)",
      },
    };
  }

  randomParams(): FunnelParams {
    const defs = this.paramDefs();
    const bottomRadius = randInt(defs.bottomRadius.range!);
    const funnelType = randChoice(defs.funnelType.choices!) as FunnelType;

    // Top radius depends on type
    let topRadius: number;
    if (funnelType === "cone") {
      topRadius = 0; // Pointed cone
    } else {
      topRadius = randInt({
        min: 2,
        max: Math.floor(bottomRadius * 0.8),
      });
    }

    return {
      bottomRadius,
      topRadius,
      height: randInt(defs.height.range!),
      funnelType,
      extensionRadius: randInt({
        min: defs.extensionRadius.range!.min,
        max: Math.min(defs.extensionRadius.range!.max, topRadius > 0 ? topRadius : bottomRadius * 0.5),
      }),
      extensionHeight: randInt(defs.extensionHeight.range!),
      wallThickness: randFloat(defs.wallThickness.range!, 1),
      plateWidth: randInt(defs.plateWidth.range!),
      plateDepth: randInt(defs.plateDepth.range!),
      plateThickness: randInt(defs.plateThickness.range!),
    };
  }

  generate(params?: Partial<FunnelParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as FunnelParams;

    // Ensure constraints
    p.topRadius = Math.min(p.topRadius, p.bottomRadius - 2);
    if (p.funnelType === "cone") p.topRadius = 0;
    if (p.topRadius < 0) p.topRadius = 0;

    if (p.funnelType === "adapter" && p.topRadius > 0) {
      p.extensionRadius = Math.min(p.extensionRadius, p.topRadius);
    }

    const lines: string[] = [];

    switch (p.funnelType) {
      case "cone":
        this.generateCone(lines, p);
        break;
      case "frustum":
        this.generateFrustum(lines, p);
        break;
      case "adapter":
        this.generateAdapter(lines, p);
        break;
      case "countersink":
        this.generateCountersink(lines, p);
        break;
      case "hopper":
        this.generateHopper(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateCone(lines: string[], p: FunnelParams): void {
    // Simple pointed cone
    // K r_bottom r_top height
    lines.push(`K ${fmt(p.bottomRadius)} 0 ${fmt(p.height)}`);
  }

  private generateFrustum(lines: string[], p: FunnelParams): void {
    // Truncated cone (frustum) - flat top
    lines.push(`K ${fmt(p.bottomRadius)} ${fmt(p.topRadius)} ${fmt(p.height)}`);
  }

  private generateAdapter(lines: string[], p: FunnelParams): void {
    // Conical adapter: frustum with cylinder extension on top
    // Ensure top radius is positive for adapter
    const topR = Math.max(p.topRadius, 5);

    // Line 0: Frustum
    lines.push(`K ${fmt(p.bottomRadius)} ${fmt(topR)} ${fmt(p.height)}`);

    // Line 1: Extension cylinder at top
    const extRadius = Math.min(p.extensionRadius, topR);
    lines.push(`Y ${fmt(extRadius)} ${fmt(p.extensionHeight)}`);

    // Line 2: Position extension on top of cone
    lines.push(`T 1 0 0 ${fmt(p.height)}`);

    // Line 3: Union
    lines.push(`U 0 2`);
  }

  private generateCountersink(lines: string[], p: FunnelParams): void {
    // Countersink: plate with conical pocket
    // Ensure dimensions make sense
    const maxConeRadius = Math.min(p.bottomRadius, p.plateWidth / 2 - 2, p.plateDepth / 2 - 2);
    const coneRadius = Math.max(5, maxConeRadius);
    const coneHeight = Math.min(p.height, p.plateThickness * 1.5);
    const topR = Math.max(1, Math.min(p.topRadius, coneRadius - 2));

    // Line 0: Base plate
    lines.push(`C ${fmt(p.plateWidth)} ${fmt(p.plateDepth)} ${fmt(p.plateThickness)}`);

    // Line 1: Cone for countersink (inverted - larger radius at top for drilling)
    // Actually we want cone opening up, so bottom is smaller hole, top is larger chamfer
    lines.push(`K ${fmt(topR)} ${fmt(coneRadius)} ${fmt(coneHeight)}`);

    // Line 2: Position cone to cut from top of plate, centered
    // Cone base at plate top minus cone height
    const zPos = p.plateThickness - coneHeight;
    lines.push(`T 1 ${fmt(p.plateWidth / 2)} ${fmt(p.plateDepth / 2)} ${fmt(zPos)}`);

    // Line 3: Subtract to create countersink
    lines.push(`D 0 2`);
  }

  private generateHopper(lines: string[], p: FunnelParams): void {
    // Hollow cone (hopper/funnel shape)
    // Outer cone minus inner cone
    const innerBottomRadius = Math.max(2, p.bottomRadius - p.wallThickness);
    const innerTopRadius = Math.max(0, p.topRadius - p.wallThickness);

    // Line 0: Outer cone
    lines.push(`K ${fmt(p.bottomRadius)} ${fmt(p.topRadius)} ${fmt(p.height)}`);

    // Line 1: Inner cone (slightly taller to ensure clean cut at top)
    lines.push(`K ${fmt(innerBottomRadius)} ${fmt(innerTopRadius)} ${fmt(p.height + 1)}`);

    // Line 2: Position inner cone slightly below (to cut through bottom)
    lines.push(`T 1 0 0 -0.5`);

    // Line 3: Subtract inner from outer
    lines.push(`D 0 2`);
  }

  private computeComplexity(p: FunnelParams): number {
    switch (p.funnelType) {
      case "cone":
        return 1;
      case "frustum":
        return 1;
      case "adapter":
        return 2;
      case "countersink":
        return 2;
      case "hopper":
        return 2;
      default:
        return 1;
    }
  }
}
