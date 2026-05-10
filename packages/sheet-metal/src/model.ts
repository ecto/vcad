/**
 * Core sheet-metal data structures (TS port of `vcad-kernel-sheet/model.rs`).
 *
 * A {@link SheetMetalModel} is a graph of {@link Panel}s (flat regions)
 * connected by {@link Bend}s (cylindrical patches). The graph is a tree
 * rooted at the reference panel; cycles will be supported when multi-body
 * welded sheet-metal lands.
 */

import type { Vec2, Vec3 } from "@vcad/ir";

export type PanelId = number;
export type BendId = number;

/**
 * 3D pose of a panel: an origin and an orthonormal basis. `x_dir` and
 * `y_dir` span the panel's mid-plane; `x_dir × y_dir` points to the
 * outside face.
 */
export interface Frame {
  origin: Vec3;
  xDir: Vec3;
  yDir: Vec3;
}

export function identityFrame(): Frame {
  return {
    origin: { x: 0, y: 0, z: 0 },
    xDir: { x: 1, y: 0, z: 0 },
    yDir: { x: 0, y: 1, z: 0 },
  };
}

export function frameNormal(f: Frame): Vec3 {
  // x × y
  return {
    x: f.xDir.y * f.yDir.z - f.xDir.z * f.yDir.y,
    y: f.xDir.z * f.yDir.x - f.xDir.x * f.yDir.z,
    z: f.xDir.x * f.yDir.y - f.xDir.y * f.yDir.x,
  };
}

export function frameToWorld(f: Frame, p: Vec2): Vec3 {
  return {
    x: f.origin.x + f.xDir.x * p.x + f.yDir.x * p.y,
    y: f.origin.y + f.xDir.y * p.x + f.yDir.y * p.y,
    z: f.origin.z + f.xDir.z * p.x + f.yDir.z * p.y,
  };
}

/**
 * A flat planar region of the sheet. Outline is in panel-local 2D coords
 * (CCW when viewed from the outside face); not closed (the first and last
 * points are not duplicated).
 */
export interface Panel {
  outline: Vec2[];
  holes: Vec2[][];
  /** 3D pose in the bent configuration. */
  frameBent: Frame;
  /** 3D pose in the unfolded (flat) configuration. */
  frameFlat: Frame;
  incidentBends: BendId[];
}

/**
 * Direction of a bend relative to the parent panel. `Up` rises out of the
 * parent's outside face; `Down` descends out of the inside face. Drives
 * red/blue DXF layer convention.
 */
export type BendDirection = "Up" | "Down";

export function bendDirectionSign(d: BendDirection): number {
  return d === "Up" ? 1 : -1;
}

/**
 * A cylindrical bend connecting two panels along a shared edge. Hinge edge
 * is in *parent-panel-local* 2D coords.
 */
export interface Bend {
  parent: PanelId;
  child: PanelId;
  /** Hinge edge in parent-local 2D coords (start, end). */
  edgeParent: [Vec2, Vec2];
  /** Inside bend radius (mm). */
  radius: number;
  /** Bend angle (radians, > 0). */
  angle: number;
  direction: BendDirection;
  /** K-factor used. */
  kFactor: number;
  /** Provenance label (e.g. `"builtin:Al-soft/R1.00t1.00"`). */
  kFactorSource: string | null;
}

/** Bend allowance: arc length of the neutral axis through this bend. */
export function bendAllowance(bend: Bend, thickness: number): number {
  return bend.angle * (bend.radius + bend.kFactor * thickness);
}

export interface SheetMetalModel {
  /** Material thickness (mm), constant across the part. */
  thickness: number;
  panels: Panel[];
  bends: Bend[];
  /** Reference panel — stays put during unfold/refold. */
  root: PanelId;
}

export function newModel(thickness: number): SheetMetalModel {
  return {
    thickness,
    panels: [],
    bends: [],
    root: 0,
  };
}

export function pushPanel(model: SheetMetalModel, panel: Panel): PanelId {
  const id = model.panels.length;
  model.panels.push(panel);
  return id;
}

export function pushBend(model: SheetMetalModel, bend: Bend): BendId {
  const id = model.bends.length;
  model.bends.push(bend);
  model.panels[bend.parent]!.incidentBends.push(id);
  model.panels[bend.child]!.incidentBends.push(id);
  return id;
}

/**
 * BFS the panel/bend graph from the root, yielding each `[panelId,
 * incomingBendId]` in order. The first item is `[root, null]`.
 */
export function* bfs(
  model: SheetMetalModel,
): Generator<[PanelId, BendId | null]> {
  if (model.panels.length === 0) return;
  const visited = new Array<boolean>(model.panels.length).fill(false);
  const queue: Array<[PanelId, BendId | null]> = [[model.root, null]];
  visited[model.root] = true;
  while (queue.length > 0) {
    const head = queue.shift()!;
    const [panel, _via] = head;
    yield head;
    for (const bendId of model.panels[panel]!.incidentBends) {
      const bend = model.bends[bendId]!;
      const other = bend.parent === panel ? bend.child : bend.parent;
      if (!visited[other]) {
        visited[other] = true;
        queue.push([other, bendId]);
      }
    }
  }
}
