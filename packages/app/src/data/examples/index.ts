import type { VcadFile, VcadFileLegacy } from "@vcad/core";
import type { Document } from "@vcad/ir";
import type { PartInfo } from "@vcad/core";

/**
 * Examples ship the legacy v0.1 IR-JSON shape inline — they predate the
 * CRDT format and there's no value in migrating the source checked-in data
 * since it gets run through `migrate_v1` on load anyway. `toVcadFile` wraps
 * them as the `legacy` variant of the tagged union.
 */
export interface ExampleFile {
  document: Document;
  parts: PartInfo[];
  consumedParts?: Record<string, PartInfo>;
  nextNodeId: number;
  nextPartNum?: number;
}

/**
 * URDF-sourced example. Loaded through the engine's `importUrdf` at click
 * time rather than inline, since the IR representation of e.g. a 23-DOF
 * humanoid is several hundred kB and the source URDF is more compact and
 * editable as text.
 */
export interface UrdfExampleSource {
  urdfText: string;
  /** Display name for toasts/logs (e.g. file name). */
  name?: string;
}

export interface Example {
  id: string;
  name: string;
  description?: string;
  difficulty?: "beginner" | "intermediate" | "advanced";
  thumbnail?: string;
  features?: string[];
  unlockAfter?: number;
  /**
   * Inline example data. Mutually exclusive with `urdf` — exactly one
   * must be set.
   */
  file?: ExampleFile;
  /** URDF source bundled via Vite `?raw`. */
  urdf?: UrdfExampleSource;
}

/** Wrap an inline example in the canonical tagged-union `VcadFile` shape. */
export function exampleToVcadFile(file: ExampleFile): VcadFile {
  const legacy: VcadFileLegacy = {
    kind: "legacy",
    version: "0.1",
    document: file.document,
    parts: file.parts,
    consumedParts: file.consumedParts,
    nextNodeId: file.nextNodeId,
    nextPartNum: file.nextPartNum,
  };
  return legacy;
}

import { plateExample } from "./plate.vcad";
import { bracketExample } from "./bracket.vcad";
import { mascotExample } from "./mascot.vcad";
import { containerExample } from "./container.vcad";
import { flangeExample } from "./flange.vcad";
import { ribbonExample } from "./ribbon.vcad";
import { springExample } from "./spring.vcad";
import { vaseExample } from "./vase.vcad";
import { wineglassExample } from "./wineglass.vcad";
import { robotArmExample } from "./robot-arm.vcad";
import { unitreeG1Example } from "./unitree-g1.urdf";
import { unitreeGo2Example } from "./unitree-go2.urdf";
import { unitreeG1OfficialExample } from "./unitree-g1-official.urdf";
import { unitreeGo2OfficialExample } from "./unitree-go2-official.urdf";

export const examples: Example[] = [
  plateExample,
  bracketExample,
  mascotExample,
  containerExample,
  flangeExample,
  ribbonExample,
  springExample,
  vaseExample,
  wineglassExample,
  robotArmExample,
  unitreeG1Example,
  unitreeGo2Example,
  unitreeG1OfficialExample,
  unitreeGo2OfficialExample,
];
