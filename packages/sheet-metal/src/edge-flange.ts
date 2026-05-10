/**
 * `addEdgeFlange` — extend an existing model with a new flange off an edge
 * of an existing panel.
 *
 * Coordinate convention: each panel's bend axis coincides with the parent's
 * edge in 3D (zero-radius idealisation). The cylindrical bend region is
 * implied metadata and only materialises during tessellation. This keeps
 * the panel graph as a clean source-of-truth and unfold/refold lossless.
 *
 * For a CCW outline, the outward in-plane direction perpendicular to edge
 * `(p, q)` is `rotate-90°-clockwise(q - p) = (dy, -dx)` (right-hand side
 * of the edge as you walk p→q).
 */

import type { Vec2, Vec3 } from "@vcad/ir";
import { vec3Normalize } from "@vcad/ir";
import { bendAllowance, lookupKFactor, type BendTable } from "./bend-table.js";
import {
  bendDirectionSign,
  frameToWorld,
  pushBend,
  pushPanel,
  type BendDirection,
  type BendId,
  type Frame,
  type PanelId,
  type SheetMetalModel,
} from "./model.js";

export type FlangePosition = "MaterialInside";

export type EdgeFlangeError =
  | { kind: "UnknownPanel"; panel: PanelId }
  | {
      kind: "EdgeOutOfRange";
      panel: PanelId;
      edgeIndex: number;
      outlineLen: number;
    }
  | {
      kind: "NonPositive";
      name: "length" | "radius" | "angle" | "edge length";
      value: number;
    }
  | { kind: "AngleTooLarge"; angle: number }
  | {
      kind: "NoKFactor";
      material: string;
      thickness: number;
      radius: number;
    };

export class EdgeFlangeException extends Error {
  constructor(public readonly detail: EdgeFlangeError) {
    super(EdgeFlangeException.message(detail));
    this.name = "EdgeFlangeException";
  }
  private static message(d: EdgeFlangeError): string {
    switch (d.kind) {
      case "UnknownPanel":
        return `unknown panel ${d.panel}`;
      case "EdgeOutOfRange":
        return `edge ${d.edgeIndex} out of range for panel ${d.panel} (outline has ${d.outlineLen} edges)`;
      case "NonPositive":
        return `${d.name} must be > 0, got ${d.value}`;
      case "AngleTooLarge":
        return `angle must be in (0, π], got ${d.angle}`;
      case "NoKFactor":
        return `no K-factor found for material=${JSON.stringify(d.material)} t=${d.thickness} R=${d.radius}`;
    }
  }
}

export interface EdgeFlangeParams {
  panel: PanelId;
  /** Index of the edge in the parent's outline (0 = outline[0]→outline[1]). */
  edgeIndex: number;
  length: number;
  /** Bend angle (radians, 0 < angle ≤ π). */
  angle: number;
  /** Inside bend radius (mm). */
  radius: number;
  direction: BendDirection;
  position: FlangePosition;
  material: string;
  /** When set, `bendTable` and `material` are ignored. */
  manualK?: number;
}

/**
 * Extend `model` with a new flange. Returns `[childPanelId, bendId]`.
 * Throws {@link EdgeFlangeException} on invalid input.
 */
export function addEdgeFlange(
  model: SheetMetalModel,
  bendTable: BendTable,
  params: EdgeFlangeParams,
): [PanelId, BendId] {
  if (params.panel >= model.panels.length || params.panel < 0) {
    throw new EdgeFlangeException({ kind: "UnknownPanel", panel: params.panel });
  }
  if (!(params.length > 0) || Number.isNaN(params.length)) {
    throw new EdgeFlangeException({
      kind: "NonPositive",
      name: "length",
      value: params.length,
    });
  }
  if (!(params.radius > 0) || Number.isNaN(params.radius)) {
    throw new EdgeFlangeException({
      kind: "NonPositive",
      name: "radius",
      value: params.radius,
    });
  }
  if (!(params.angle > 0) || Number.isNaN(params.angle)) {
    throw new EdgeFlangeException({
      kind: "NonPositive",
      name: "angle",
      value: params.angle,
    });
  }
  if (params.angle > Math.PI + 1e-12) {
    throw new EdgeFlangeException({
      kind: "AngleTooLarge",
      angle: params.angle,
    });
  }

  const parent = model.panels[params.panel]!;
  const n = parent.outline.length;
  if (n < 3 || params.edgeIndex >= n || params.edgeIndex < 0) {
    throw new EdgeFlangeException({
      kind: "EdgeOutOfRange",
      panel: params.panel,
      edgeIndex: params.edgeIndex,
      outlineLen: n,
    });
  }

  // Resolve K-factor — manual override beats table lookup.
  let kFactor: number;
  let sourceLabel: string;
  if (params.manualK !== undefined) {
    kFactor = params.manualK;
    sourceLabel = "manual";
  } else {
    const looked = lookupKFactor(
      bendTable,
      params.material,
      model.thickness,
      params.radius,
    );
    if (looked === null) {
      throw new EdgeFlangeException({
        kind: "NoKFactor",
        material: params.material,
        thickness: model.thickness,
        radius: params.radius,
      });
    }
    kFactor = looked.kFactor;
    sourceLabel =
      looked.source.kind === "Builtin" ? `builtin:${looked.source.key}` : "shop";
  }

  const p0 = parent.outline[params.edgeIndex]!;
  const p1 = parent.outline[(params.edgeIndex + 1) % n]!;
  const edgeVec: Vec2 = { x: p1.x - p0.x, y: p1.y - p0.y };
  const edgeLen = Math.hypot(edgeVec.x, edgeVec.y);
  if (edgeLen < 1e-12) {
    throw new EdgeFlangeException({
      kind: "NonPositive",
      name: "edge length",
      value: edgeLen,
    });
  }
  const edgeDir2D: Vec2 = { x: edgeVec.x / edgeLen, y: edgeVec.y / edgeLen };
  // Outward normal of CCW edge: rotate edge_dir 90° clockwise.
  const outward2D: Vec2 = { x: edgeDir2D.y, y: -edgeDir2D.x };

  const parentFrame = parent.frameBent;
  const edgeDir3D = direction2DToWorld(parentFrame, edgeDir2D);
  const outward3D = direction2DToWorld(parentFrame, outward2D);

  const signedAngle = bendDirectionSign(params.direction) * params.angle;
  const axis = vec3Normalize(edgeDir3D);
  const childYDirBent = rotateVecAboutAxis(outward3D, axis, signedAngle);
  const childOriginBent = frameToWorld(parentFrame, p0);

  const childFrameBent: Frame = {
    origin: childOriginBent,
    xDir: edgeDir3D,
    yDir: childYDirBent,
  };

  // Flat pose: separated from the hinge by the bend allowance, no rotation.
  const ba = bendAllowance(params.angle, params.radius, kFactor, model.thickness);
  const parentFlatHinge = frameToWorld(parent.frameFlat, p0);
  const outwardFlat3D = direction2DToWorld(parent.frameFlat, outward2D);
  const childOriginFlat: Vec3 = {
    x: parentFlatHinge.x + outwardFlat3D.x * ba,
    y: parentFlatHinge.y + outwardFlat3D.y * ba,
    z: parentFlatHinge.z + outwardFlat3D.z * ba,
  };
  const childFrameFlat: Frame = {
    origin: childOriginFlat,
    xDir: direction2DToWorld(parent.frameFlat, edgeDir2D),
    yDir: outwardFlat3D,
  };

  const childOutline: Vec2[] = [
    { x: 0, y: 0 },
    { x: edgeLen, y: 0 },
    { x: edgeLen, y: params.length },
    { x: 0, y: params.length },
  ];

  const childId = pushPanel(model, {
    outline: childOutline,
    holes: [],
    frameBent: childFrameBent,
    frameFlat: childFrameFlat,
    incidentBends: [],
  });

  const bendId = pushBend(model, {
    parent: params.panel,
    child: childId,
    edgeParent: [p0, p1],
    radius: params.radius,
    angle: params.angle,
    direction: params.direction,
    kFactor,
    kFactorSource: sourceLabel,
  });

  return [childId, bendId];
}

/**
 * Lift a panel-local 2D direction into world 3D using `frame`. (Like
 * `frameToWorld` but for direction vectors — no origin translation.)
 */
function direction2DToWorld(frame: Frame, d: Vec2): Vec3 {
  return {
    x: frame.xDir.x * d.x + frame.yDir.x * d.y,
    y: frame.xDir.y * d.x + frame.yDir.y * d.y,
    z: frame.xDir.z * d.x + frame.yDir.z * d.y,
  };
}

/**
 * Rotate vector `v` about a unit `axis` by `angle` radians (Rodrigues).
 */
export function rotateVecAboutAxis(
  v: Vec3,
  axis: Vec3,
  angle: number,
): Vec3 {
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const t = 1 - c;
  const dot = v.x * axis.x + v.y * axis.y + v.z * axis.z;
  // Rodrigues: v cosθ + (k×v) sinθ + k (k·v)(1-cosθ)
  const cross: Vec3 = {
    x: axis.y * v.z - axis.z * v.y,
    y: axis.z * v.x - axis.x * v.z,
    z: axis.x * v.y - axis.y * v.x,
  };
  return {
    x: v.x * c + cross.x * s + axis.x * dot * t,
    y: v.y * c + cross.y * s + axis.y * dot * t,
    z: v.z * c + cross.z * s + axis.z * dot * t,
  };
}
