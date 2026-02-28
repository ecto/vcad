/**
 * Mount generator - motor mounts, sensor brackets, and similar mounting hardware.
 *
 * Generates:
 * - Motor mounts (NEMA-style patterns)
 * - Sensor mounts (simple brackets)
 * - Adjustable mounts (slotted holes)
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
  cornerHoles,
  type HolePosition,
} from "./utils.js";

export type MountType = "nema17" | "nema23" | "sensor" | "adjustable";

export interface MountParams extends PartParams {
  mountType: MountType;
  plateWidth: number;
  plateHeight: number;
  plateThickness: number;
  centerHoleDiameter: number;
  mountingHoleDiameter: number;
  boltPattern: number; // Distance between bolt holes (for NEMA)
  sensorDiameter: number;
  slotLength: number;
  slotWidth: number;
  hasBoss: boolean;
  bossDiameter: number;
  bossHeight: number;
}

export class MountGenerator implements PartGenerator {
  readonly family = "mount";
  readonly description = "Motor mounts, sensor brackets, and adjustable mounts";

  paramDefs(): Record<string, ParamDef> {
    return {
      mountType: {
        type: "choice",
        choices: ["nema17", "nema23", "sensor", "adjustable"],
        description: "Mount type",
      },
      plateWidth: {
        type: "number",
        range: { min: 30, max: 80, step: 5 },
        description: "Plate width (mm)",
      },
      plateHeight: {
        type: "number",
        range: { min: 30, max: 80, step: 5 },
        description: "Plate height (mm)",
      },
      plateThickness: {
        type: "number",
        range: { min: 3, max: 8, step: 0.5 },
        description: "Plate thickness (mm)",
      },
      centerHoleDiameter: {
        type: "number",
        range: { min: 15, max: 40, step: 1 },
        description: "Center opening diameter (mm)",
      },
      mountingHoleDiameter: {
        type: "number",
        range: { min: 3, max: 6, step: 0.5 },
        description: "Mounting hole diameter (mm)",
      },
      boltPattern: {
        type: "number",
        range: { min: 25, max: 50, step: 5 },
        description: "Bolt pattern distance (mm)",
      },
      sensorDiameter: {
        type: "number",
        range: { min: 8, max: 20, step: 1 },
        description: "Sensor body diameter (mm)",
      },
      slotLength: {
        type: "number",
        range: { min: 10, max: 25, step: 2 },
        description: "Adjustment slot length (mm)",
      },
      slotWidth: {
        type: "number",
        range: { min: 4, max: 8, step: 0.5 },
        description: "Adjustment slot width (mm)",
      },
      hasBoss: {
        type: "boolean",
        description: "Include centering boss",
      },
      bossDiameter: {
        type: "number",
        range: { min: 20, max: 30, step: 2 },
        description: "Centering boss diameter (mm)",
      },
      bossHeight: {
        type: "number",
        range: { min: 2, max: 5, step: 0.5 },
        description: "Centering boss height (mm)",
      },
    };
  }

  randomParams(): MountParams {
    const defs = this.paramDefs();
    const mountType = randChoice(defs.mountType.choices!) as MountType;

    // Set typical dimensions based on mount type
    let plateWidth: number, plateHeight: number, boltPattern: number, centerHole: number;

    switch (mountType) {
      case "nema17":
        plateWidth = 50;
        plateHeight = 50;
        boltPattern = 31; // NEMA 17 standard
        centerHole = 22;
        break;
      case "nema23":
        plateWidth = 65;
        plateHeight = 65;
        boltPattern = 47; // NEMA 23 standard
        centerHole = 38;
        break;
      default:
        plateWidth = randInt(defs.plateWidth.range!);
        plateHeight = randInt(defs.plateHeight.range!);
        boltPattern = randInt(defs.boltPattern.range!);
        centerHole = randInt(defs.centerHoleDiameter.range!);
    }

    return {
      mountType,
      plateWidth,
      plateHeight,
      plateThickness: randFloat(defs.plateThickness.range!, 1),
      centerHoleDiameter: centerHole,
      mountingHoleDiameter: randFloat(defs.mountingHoleDiameter.range!, 1),
      boltPattern,
      sensorDiameter: randInt(defs.sensorDiameter.range!),
      slotLength: randInt(defs.slotLength.range!),
      slotWidth: randFloat(defs.slotWidth.range!, 1),
      hasBoss: randBool(0.4),
      bossDiameter: randInt(defs.bossDiameter.range!),
      bossHeight: randFloat(defs.bossHeight.range!, 1),
    };
  }

  generate(params?: Partial<MountParams>): GeneratedPart {
    const p = { ...this.randomParams(), ...params } as MountParams;

    // Ensure constraints
    p.bossDiameter = Math.min(p.bossDiameter, p.centerHoleDiameter + 10);

    const lines: string[] = [];
    let baseIdx = 0;

    // Base plate
    lines.push(`C ${fmt(p.plateWidth)} ${fmt(p.plateHeight)} ${fmt(p.plateThickness)}`);

    // Add centering boss if requested
    if (p.hasBoss && (p.mountType === "nema17" || p.mountType === "nema23")) {
      baseIdx = this.addBoss(lines, baseIdx, p);
    }

    // Add center hole
    baseIdx = this.addCenterHole(lines, baseIdx, p);

    // Add mounting holes based on type
    switch (p.mountType) {
      case "nema17":
      case "nema23":
        baseIdx = this.addNemaPattern(lines, baseIdx, p);
        break;
      case "sensor":
        baseIdx = this.addSensorHoles(lines, baseIdx, p);
        break;
      case "adjustable":
        baseIdx = this.addAdjustmentSlots(lines, baseIdx, p);
        break;
    }

    return {
      vcode: lines.join("\n"),
      params: p,
      family: this.family,
      complexity: this.computeComplexity(p),
    };
  }

  private addBoss(lines: string[], baseIdx: number, p: MountParams): number {
    const bossIdx = lines.length;
    lines.push(`Y ${fmt(p.bossDiameter / 2)} ${fmt(p.bossHeight)}`);

    // Position at center, on top of plate
    const transIdx = lines.length;
    lines.push(
      `T ${bossIdx} ${fmt(p.plateWidth / 2)} ${fmt(p.plateHeight / 2)} ${fmt(p.plateThickness)}`,
    );

    const unionIdx = lines.length;
    lines.push(`U ${baseIdx} ${transIdx}`);

    return unionIdx;
  }

  private addCenterHole(lines: string[], baseIdx: number, p: MountParams): number {
    const totalHeight = p.hasBoss ? p.plateThickness + p.bossHeight : p.plateThickness;

    const holeIdx = lines.length;
    lines.push(`Y ${fmt(p.centerHoleDiameter / 2)} ${fmt(totalHeight + 2)}`);

    const transIdx = lines.length;
    lines.push(`T ${holeIdx} ${fmt(p.plateWidth / 2)} ${fmt(p.plateHeight / 2)} -1`);

    const diffIdx = lines.length;
    lines.push(`D ${baseIdx} ${transIdx}`);

    return diffIdx;
  }

  private addNemaPattern(lines: string[], baseIdx: number, p: MountParams): number {
    // NEMA motors have 4 holes in a square pattern
    const halfPattern = p.boltPattern / 2;
    const centerX = p.plateWidth / 2;
    const centerY = p.plateHeight / 2;

    const holes: HolePosition[] = [
      { x: centerX - halfPattern, y: centerY - halfPattern },
      { x: centerX + halfPattern, y: centerY - halfPattern },
      { x: centerX + halfPattern, y: centerY + halfPattern },
      { x: centerX - halfPattern, y: centerY + halfPattern },
    ];

    let currentBase = baseIdx;

    for (const hole of holes) {
      const cylIdx = lines.length;
      lines.push(`Y ${fmt(p.mountingHoleDiameter / 2)} ${fmt(p.plateThickness + 2)}`);

      const transIdx = lines.length;
      lines.push(`T ${cylIdx} ${fmt(hole.x)} ${fmt(hole.y)} -1`);

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private addSensorHoles(lines: string[], baseIdx: number, p: MountParams): number {
    // Two mounting holes on either side of the sensor
    const holeSpacing = p.sensorDiameter + 10;
    const centerY = p.plateHeight / 2;

    const holes: HolePosition[] = [
      { x: p.plateWidth / 2 - holeSpacing / 2, y: centerY },
      { x: p.plateWidth / 2 + holeSpacing / 2, y: centerY },
    ];

    let currentBase = baseIdx;

    for (const hole of holes) {
      const cylIdx = lines.length;
      lines.push(`Y ${fmt(p.mountingHoleDiameter / 2)} ${fmt(p.plateThickness + 2)}`);

      const transIdx = lines.length;
      lines.push(`T ${cylIdx} ${fmt(hole.x)} ${fmt(hole.y)} -1`);

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private addAdjustmentSlots(lines: string[], baseIdx: number, p: MountParams): number {
    // Two slots on either side for adjustment
    const slotSpacing = p.plateWidth * 0.6;
    const centerY = p.plateHeight / 2;

    let currentBase = baseIdx;

    for (let side = 0; side < 2; side++) {
      const slotX = p.plateWidth / 2 + (side === 0 ? -slotSpacing / 2 : slotSpacing / 2);

      // Slot is made from a cube (rectangular portion) + two cylinders (rounded ends)
      // Simplified: just use a cube for now
      const slotIdx = lines.length;
      lines.push(`C ${fmt(p.slotWidth)} ${fmt(p.slotLength)} ${fmt(p.plateThickness + 2)}`);

      const transIdx = lines.length;
      lines.push(
        `T ${slotIdx} ${fmt(slotX - p.slotWidth / 2)} ${fmt(centerY - p.slotLength / 2)} -1`,
      );

      const diffIdx = lines.length;
      lines.push(`D ${currentBase} ${transIdx}`);
      currentBase = diffIdx;
    }

    return currentBase;
  }

  private computeComplexity(p: MountParams): number {
    let complexity = 2; // Base plate with center hole
    if (p.hasBoss) complexity++;
    if (p.mountType === "nema17" || p.mountType === "nema23") complexity++;
    if (p.mountType === "adjustable") complexity++;
    return Math.min(complexity, 4);
  }
}
