/**
 * Profile generator - parts using SK (Sketch) + E (Extrude) operations.
 *
 * Generates:
 * - L-channel (L-shaped profile)
 * - T-slot (T-shaped profile)
 * - C-channel (U-shaped profile)
 * - I-beam (H-shaped profile)
 * - Custom polygon
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

export type ProfileType = "lChannel" | "tSlot" | "cChannel" | "iBeam" | "polygon";

export interface ProfileParams extends PartParams {
  profileType: ProfileType;
  /** Extrusion length */
  length: number;
  /** Profile width */
  width: number;
  /** Profile height */
  height: number;
  /** Wall/flange thickness */
  thickness: number;
  /** For I-beam: flange width */
  flangeWidth: number;
  /** For polygon: number of sides */
  sides: number;
  /** For polygon: outer radius */
  polyRadius: number;
}

export class ProfileGenerator implements PartGenerator {
  readonly family = "profile";
  readonly description = "Parts using SK (Sketch) + E (Extrude) operations";

  paramDefs(): Record<string, ParamDef> {
    return {
      profileType: {
        type: "choice",
        choices: ["lChannel", "tSlot", "cChannel", "iBeam", "polygon"],
        description: "Type of extruded profile",
      },
      length: {
        type: "number",
        range: { min: 20, max: 150, step: 5 },
        description: "Extrusion length (mm)",
      },
      width: {
        type: "number",
        range: { min: 15, max: 60, step: 5 },
        description: "Profile width (mm)",
      },
      height: {
        type: "number",
        range: { min: 15, max: 60, step: 5 },
        description: "Profile height (mm)",
      },
      thickness: {
        type: "number",
        range: { min: 2, max: 8, step: 1 },
        description: "Wall thickness (mm)",
      },
      flangeWidth: {
        type: "number",
        range: { min: 10, max: 40, step: 2 },
        description: "I-beam flange width (mm)",
      },
      sides: {
        type: "number",
        range: { min: 5, max: 8, step: 1 },
        description: "Polygon number of sides",
      },
      polyRadius: {
        type: "number",
        range: { min: 10, max: 40, step: 2 },
        description: "Polygon outer radius (mm)",
      },
    };
  }

  randomParams(): ProfileParams {
    const defs = this.paramDefs();
    return {
      profileType: randChoice(defs.profileType.choices!) as ProfileType,
      length: randInt(defs.length.range!),
      width: randInt(defs.width.range!),
      height: randInt(defs.height.range!),
      thickness: randInt(defs.thickness.range!),
      flangeWidth: randInt(defs.flangeWidth.range!),
      sides: randInt(defs.sides.range!),
      polyRadius: randInt(defs.polyRadius.range!),
    };
  }

  generate(params?: Partial<ProfileParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as ProfileParams;

    // Ensure constraints
    p.thickness = Math.min(p.thickness, p.width / 3, p.height / 3);

    const lines: string[] = [];

    switch (p.profileType) {
      case "lChannel":
        this.generateLChannel(lines, p);
        break;
      case "tSlot":
        this.generateTSlot(lines, p);
        break;
      case "cChannel":
        this.generateCChannel(lines, p);
        break;
      case "iBeam":
        this.generateIBeam(lines, p);
        break;
      case "polygon":
        this.generatePolygon(lines, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private generateLChannel(lines: string[], p: ProfileParams): void {
    // L-channel: two perpendicular walls
    // Sketch on XY plane, extrude along Z
    //
    // Profile shape:
    //   ┌──┐
    //   │  │
    //   │  └────┐
    //   └───────┘

    const t = p.thickness;
    const w = p.width;
    const h = p.height;

    // SK origin x_dir y_dir (sketch on XY plane at origin)
    lines.push(`SK 0 0 0  1 0 0  0 1 0`);

    // Draw L shape counter-clockwise
    // Start at origin (0,0), go around
    lines.push(`L 0 0 ${fmt(w)} 0`);          // Bottom edge
    lines.push(`L ${fmt(w)} 0 ${fmt(w)} ${fmt(t)}`);  // Right short edge
    lines.push(`L ${fmt(w)} ${fmt(t)} ${fmt(t)} ${fmt(t)}`);  // Inner horizontal
    lines.push(`L ${fmt(t)} ${fmt(t)} ${fmt(t)} ${fmt(h)}`);  // Inner vertical
    lines.push(`L ${fmt(t)} ${fmt(h)} 0 ${fmt(h)}`);  // Top edge
    lines.push(`L 0 ${fmt(h)} 0 0`);          // Left edge (close)
    lines.push(`END`);

    // Extrude along Z (node 0 is the sketch)
    lines.push(`E 0 0 0 ${fmt(p.length)}`);
  }

  private generateTSlot(lines: string[], p: ProfileParams): void {
    // T-slot profile: T-shape for aluminum extrusion
    //
    //   ┌─────────┐
    //   └──┐   ┌──┘
    //      │   │
    //      └───┘

    const t = p.thickness;
    const w = p.width;
    const h = p.height;
    const stemWidth = t * 1.5;
    const flangeHeight = t;

    // Ensure stem is centered
    const stemLeft = (w - stemWidth) / 2;
    const stemRight = stemLeft + stemWidth;

    lines.push(`SK 0 0 0  1 0 0  0 1 0`);

    // Draw T shape
    lines.push(`L 0 ${fmt(h - flangeHeight)} 0 ${fmt(h)}`);  // Left flange outer
    lines.push(`L 0 ${fmt(h)} ${fmt(w)} ${fmt(h)}`);  // Top edge
    lines.push(`L ${fmt(w)} ${fmt(h)} ${fmt(w)} ${fmt(h - flangeHeight)}`);  // Right flange outer
    lines.push(`L ${fmt(w)} ${fmt(h - flangeHeight)} ${fmt(stemRight)} ${fmt(h - flangeHeight)}`);  // Right step
    lines.push(`L ${fmt(stemRight)} ${fmt(h - flangeHeight)} ${fmt(stemRight)} 0`);  // Right stem
    lines.push(`L ${fmt(stemRight)} 0 ${fmt(stemLeft)} 0`);  // Bottom
    lines.push(`L ${fmt(stemLeft)} 0 ${fmt(stemLeft)} ${fmt(h - flangeHeight)}`);  // Left stem
    lines.push(`L ${fmt(stemLeft)} ${fmt(h - flangeHeight)} 0 ${fmt(h - flangeHeight)}`);  // Left step (close)
    lines.push(`END`);

    lines.push(`E 0 0 0 ${fmt(p.length)}`);
  }

  private generateCChannel(lines: string[], p: ProfileParams): void {
    // C-channel (U-channel): three sides of a rectangle
    //
    //   ┌──┐     ┌──┐
    //   │  └─────┘  │
    //   │           │
    //   └───────────┘

    const t = p.thickness;
    const w = p.width;
    const h = p.height;

    lines.push(`SK 0 0 0  1 0 0  0 1 0`);

    // Draw C shape (U rotated 90°)
    lines.push(`L 0 0 ${fmt(w)} 0`);  // Bottom
    lines.push(`L ${fmt(w)} 0 ${fmt(w)} ${fmt(h)}`);  // Right outer
    lines.push(`L ${fmt(w)} ${fmt(h)} ${fmt(w - t)} ${fmt(h)}`);  // Right top
    lines.push(`L ${fmt(w - t)} ${fmt(h)} ${fmt(w - t)} ${fmt(t)}`);  // Right inner
    lines.push(`L ${fmt(w - t)} ${fmt(t)} ${fmt(t)} ${fmt(t)}`);  // Inner bottom
    lines.push(`L ${fmt(t)} ${fmt(t)} ${fmt(t)} ${fmt(h)}`);  // Left inner
    lines.push(`L ${fmt(t)} ${fmt(h)} 0 ${fmt(h)}`);  // Left top
    lines.push(`L 0 ${fmt(h)} 0 0`);  // Left outer (close)
    lines.push(`END`);

    lines.push(`E 0 0 0 ${fmt(p.length)}`);
  }

  private generateIBeam(lines: string[], p: ProfileParams): void {
    // I-beam (H-beam): two flanges connected by web
    //
    //   ┌─────────┐
    //   └──┐   ┌──┘
    //      │   │
    //   ┌──┘   └──┐
    //   └─────────┘

    const t = p.thickness;
    const w = p.flangeWidth;
    const h = p.height;
    const webThickness = t;

    const webLeft = (w - webThickness) / 2;
    const webRight = webLeft + webThickness;

    lines.push(`SK 0 0 0  1 0 0  0 1 0`);

    // Draw I shape
    lines.push(`L 0 0 ${fmt(w)} 0`);  // Bottom flange bottom
    lines.push(`L ${fmt(w)} 0 ${fmt(w)} ${fmt(t)}`);  // Bottom flange right
    lines.push(`L ${fmt(w)} ${fmt(t)} ${fmt(webRight)} ${fmt(t)}`);  // Bottom right step
    lines.push(`L ${fmt(webRight)} ${fmt(t)} ${fmt(webRight)} ${fmt(h - t)}`);  // Web right
    lines.push(`L ${fmt(webRight)} ${fmt(h - t)} ${fmt(w)} ${fmt(h - t)}`);  // Top right step
    lines.push(`L ${fmt(w)} ${fmt(h - t)} ${fmt(w)} ${fmt(h)}`);  // Top flange right
    lines.push(`L ${fmt(w)} ${fmt(h)} 0 ${fmt(h)}`);  // Top flange top
    lines.push(`L 0 ${fmt(h)} 0 ${fmt(h - t)}`);  // Top flange left
    lines.push(`L 0 ${fmt(h - t)} ${fmt(webLeft)} ${fmt(h - t)}`);  // Top left step
    lines.push(`L ${fmt(webLeft)} ${fmt(h - t)} ${fmt(webLeft)} ${fmt(t)}`);  // Web left
    lines.push(`L ${fmt(webLeft)} ${fmt(t)} 0 ${fmt(t)}`);  // Bottom left step
    lines.push(`L 0 ${fmt(t)} 0 0`);  // Bottom flange left (close)
    lines.push(`END`);

    lines.push(`E 0 0 0 ${fmt(p.length)}`);
  }

  private generatePolygon(lines: string[], p: ProfileParams): void {
    // Regular polygon extruded
    // Ensure sides is an integer >= 3 (in case of conversation modifications)
    const n = Math.max(3, Math.round(p.sides || 5));
    const r = p.polyRadius;

    lines.push(`SK 0 0 0  1 0 0  0 1 0`);

    // Generate polygon vertices
    const vertices: Array<{ x: number; y: number }> = [];
    for (let i = 0; i < n; i++) {
      const angle = (2 * Math.PI * i) / n - Math.PI / 2; // Start at top
      vertices.push({
        x: r * Math.cos(angle) + r, // Offset to positive quadrant
        y: r * Math.sin(angle) + r,
      });
    }

    // Draw edges
    for (let i = 0; i < n; i++) {
      const curr = vertices[i];
      const next = vertices[(i + 1) % n];
      lines.push(`L ${fmt(curr.x)} ${fmt(curr.y)} ${fmt(next.x)} ${fmt(next.y)}`);
    }
    lines.push(`END`);

    lines.push(`E 0 0 0 ${fmt(p.length)}`);
  }

  private computeComplexity(p: ProfileParams): number {
    switch (p.profileType) {
      case "lChannel":
        return 2;
      case "tSlot":
        return 3;
      case "cChannel":
        return 2;
      case "iBeam":
        return 3;
      case "polygon":
        return (p.sides || 5) <= 6 ? 2 : 3;
      default:
        return 2;
    }
  }
}
