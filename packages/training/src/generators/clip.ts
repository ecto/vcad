/**
 * Clip generator - parts using the I (Intersection) operation.
 *
 * Generates:
 * - Pipe saddle (cylinder ∩ cylinder)
 * - Rounded block (cube ∩ sphere)
 * - Quarter-round trim (cylinder ∩ cube)
 * - Channel/groove (cube ∩ translated cube)
 * - Lens shape (sphere ∩ sphere)
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

export type ClipType = "saddle" | "rounded" | "quarter" | "channel" | "lens";

export interface ClipParams extends PartParams {
  clipType: ClipType;
  /** Primary dimension (varies by type) */
  size1: number;
  /** Secondary dimension */
  size2: number;
  /** Tertiary dimension */
  size3: number;
  /** For saddle: pipe diameter */
  pipeDiameter: number;
  /** For saddle: saddle width */
  saddleWidth: number;
  /** For rounded: corner radius ratio (0.1-0.9) */
  roundingRatio: number;
  /** For quarter: which quadrant (0-3) */
  quadrant: number;
  /** For channel: channel depth ratio */
  channelDepth: number;
  /** For lens: second sphere radius */
  lensRadius2: number;
  /** For lens: sphere offset */
  lensOffset: number;
}

export class ClipGenerator implements PartGenerator {
  readonly family = "clip";
  readonly description = "Parts using I (Intersection) operation";

  paramDefs(): Record<string, ParamDef> {
    return {
      clipType: {
        type: "choice",
        choices: ["saddle", "rounded", "quarter", "channel", "lens"],
        description: "Type of intersection-based part",
      },
      size1: {
        type: "number",
        range: { min: 10, max: 80, step: 2 },
        description: "Primary dimension (mm)",
      },
      size2: {
        type: "number",
        range: { min: 10, max: 80, step: 2 },
        description: "Secondary dimension (mm)",
      },
      size3: {
        type: "number",
        range: { min: 5, max: 60, step: 2 },
        description: "Tertiary dimension (mm)",
      },
      pipeDiameter: {
        type: "number",
        range: { min: 10, max: 60, step: 2 },
        description: "Pipe diameter for saddle (mm)",
      },
      saddleWidth: {
        type: "number",
        range: { min: 15, max: 50, step: 2 },
        description: "Saddle width (mm)",
      },
      roundingRatio: {
        type: "number",
        range: { min: 0.2, max: 0.8, step: 0.1 },
        description: "Corner rounding ratio",
      },
      quadrant: {
        type: "number",
        range: { min: 0, max: 3, step: 1 },
        description: "Quarter-round quadrant (0-3)",
      },
      channelDepth: {
        type: "number",
        range: { min: 0.2, max: 0.8, step: 0.1 },
        description: "Channel depth as ratio of size",
      },
      lensRadius2: {
        type: "number",
        range: { min: 10, max: 60, step: 2 },
        description: "Second sphere radius for lens (mm)",
      },
      lensOffset: {
        type: "number",
        range: { min: 5, max: 40, step: 2 },
        description: "Sphere offset for lens shape (mm)",
      },
    };
  }

  randomParams(): ClipParams {
    const defs = this.paramDefs();
    return {
      clipType: randChoice(defs.clipType.choices!) as ClipType,
      size1: randInt(defs.size1.range!),
      size2: randInt(defs.size2.range!),
      size3: randInt(defs.size3.range!),
      pipeDiameter: randInt(defs.pipeDiameter.range!),
      saddleWidth: randInt(defs.saddleWidth.range!),
      roundingRatio: randFloat(defs.roundingRatio.range!, 1),
      quadrant: randInt(defs.quadrant.range!),
      channelDepth: randFloat(defs.channelDepth.range!, 1),
      lensRadius2: randInt(defs.lensRadius2.range!),
      lensOffset: randInt(defs.lensOffset.range!),
    };
  }

  generate(params?: Partial<ClipParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as ClipParams;

    const lines: string[] = [];

    switch (p.clipType) {
      case "saddle":
        this.generateSaddle(lines, p);
        break;
      case "rounded":
        this.generateRounded(lines, p);
        break;
      case "quarter":
        this.generateQuarter(lines, p);
        break;
      case "channel":
        this.generateChannel(lines, p);
        break;
      case "lens":
        this.generateLens(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateSaddle(lines: string[], p: ClipParams): void {
    // Pipe saddle: intersection of two cylinders at 90 degrees
    // Used for clamping/supporting pipes
    const pipeRadius = p.pipeDiameter / 2;
    const saddleRadius = pipeRadius * 1.2; // Slightly larger to wrap around

    // Line 0: Main cylinder (the saddle body, horizontal along X)
    lines.push(`Y ${fmt(saddleRadius)} ${fmt(p.saddleWidth)}`);

    // Line 1: Rotate to horizontal
    lines.push(`R 0 0 90 0`);

    // Line 2: Pipe cylinder (vertical, for cutting the saddle shape)
    lines.push(`Y ${fmt(pipeRadius)} ${fmt(p.saddleWidth * 2)}`);

    // Line 3: Position pipe cylinder centered
    lines.push(`T 2 ${fmt(p.saddleWidth / 2)} 0 ${fmt(-p.saddleWidth)}`);

    // Line 4: Intersection creates saddle
    lines.push(`I 1 3`);
  }

  private generateRounded(lines: string[], p: ClipParams): void {
    // Rounded block: cube intersected with sphere for smooth corners
    // Creates a "pillow" or "cushion" shape

    // Calculate sphere radius to achieve desired rounding
    // Sphere needs to be large enough to enclose the cube diagonally
    const maxDim = Math.max(p.size1, p.size2, p.size3);
    const sphereRadius = maxDim * (0.5 + p.roundingRatio);

    // Line 0: Cube
    lines.push(`C ${fmt(p.size1)} ${fmt(p.size2)} ${fmt(p.size3)}`);

    // Line 1: Sphere for rounding
    lines.push(`S ${fmt(sphereRadius)}`);

    // Line 2: Center sphere on cube
    lines.push(`T 1 ${fmt(p.size1 / 2)} ${fmt(p.size2 / 2)} ${fmt(p.size3 / 2)}`);

    // Line 3: Intersection
    lines.push(`I 0 2`);
  }

  private generateQuarter(lines: string[], p: ClipParams): void {
    // Quarter-round trim: cylinder intersected with cube to get 1/4 arc
    // Common in molding/trim pieces
    const radius = Math.min(p.size1, p.size2);
    const length = p.size3;

    // Line 0: Cylinder (full circle)
    lines.push(`Y ${fmt(radius)} ${fmt(length)}`);

    // Line 1: Cube for selecting quadrant
    lines.push(`C ${fmt(radius + 1)} ${fmt(radius + 1)} ${fmt(length + 2)}`);

    // Line 2: Position cube based on quadrant
    // Quadrant 0: +X, +Y  |  1: -X, +Y  |  2: -X, -Y  |  3: +X, -Y
    let offsetX = 0;
    let offsetY = 0;
    switch (p.quadrant) {
      case 0:
        offsetX = 0;
        offsetY = 0;
        break;
      case 1:
        offsetX = -radius - 1;
        offsetY = 0;
        break;
      case 2:
        offsetX = -radius - 1;
        offsetY = -radius - 1;
        break;
      case 3:
        offsetX = 0;
        offsetY = -radius - 1;
        break;
    }
    lines.push(`T 1 ${fmt(offsetX)} ${fmt(offsetY)} -1`);

    // Line 3: Intersection for quarter round
    lines.push(`I 0 2`);
  }

  private generateChannel(lines: string[], p: ClipParams): void {
    // Channel: intersection of two offset cubes creates a step/channel
    // This creates an L-shaped or step profile
    const channelCut = p.size2 * p.channelDepth;

    // Line 0: Main block
    lines.push(`C ${fmt(p.size1)} ${fmt(p.size2)} ${fmt(p.size3)}`);

    // Line 1: Second block for intersection (creates step)
    const block2Width = p.size1 * 0.7;
    const block2Depth = p.size2 - channelCut + 1;
    lines.push(`C ${fmt(block2Width)} ${fmt(block2Depth)} ${fmt(p.size3 + 2)}`);

    // Line 2: Position second block to create channel along one edge
    lines.push(`T 1 ${fmt((p.size1 - block2Width) / 2)} 0 -1`);

    // Line 3: Intersection creates channeled block
    lines.push(`I 0 2`);
  }

  private generateLens(lines: string[], p: ClipParams): void {
    // Lens shape: intersection of two spheres
    // Creates convex-convex lens (like a magnifying lens cross-section)
    const r1 = p.size1;
    const r2 = p.lensRadius2;

    // Calculate offset so spheres overlap appropriately
    // Offset should be less than r1 + r2 for intersection to exist
    const maxOffset = r1 + r2 - 5;
    const offset = Math.min(p.lensOffset, maxOffset);

    // Line 0: First sphere
    lines.push(`S ${fmt(r1)}`);

    // Line 1: Second sphere
    lines.push(`S ${fmt(r2)}`);

    // Line 2: Offset second sphere along Z axis
    lines.push(`T 1 0 0 ${fmt(offset)}`);

    // Line 3: Intersection creates lens
    lines.push(`I 0 2`);
  }

  private computeComplexity(p: ClipParams): number {
    // All intersection-based parts are moderate complexity
    return 2;
  }
}
