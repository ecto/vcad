/**
 * `SheetMetalModel` constructors — the "base flange" operations.
 */

import type { Vec2 } from "@vcad/ir";
import {
  identityFrame,
  newModel,
  pushPanel,
  type SheetMetalModel,
} from "./model.js";

export type BaseFlangeError =
  | { kind: "InvalidThickness"; thickness: number }
  | { kind: "OutlineTooSmall"; n: number }
  | { kind: "NonPositiveDimension"; name: "width" | "depth"; value: number };

export class BaseFlangeException extends Error {
  constructor(public readonly detail: BaseFlangeError) {
    super(BaseFlangeException.message(detail));
    this.name = "BaseFlangeException";
  }
  private static message(d: BaseFlangeError): string {
    switch (d.kind) {
      case "InvalidThickness":
        return `thickness must be > 0, got ${d.thickness}`;
      case "OutlineTooSmall":
        return `outline needs >= 3 points, got ${d.n}`;
      case "NonPositiveDimension":
        return `${d.name} must be > 0, got ${d.value}`;
    }
  }
}

/**
 * Build a sheet-metal model from a closed polygon outline in the XY plane.
 */
export function baseFlangePolygon(
  outline: Vec2[],
  thickness: number,
): SheetMetalModel {
  if (!(thickness > 0) || Number.isNaN(thickness)) {
    throw new BaseFlangeException({ kind: "InvalidThickness", thickness });
  }
  if (outline.length < 3) {
    throw new BaseFlangeException({
      kind: "OutlineTooSmall",
      n: outline.length,
    });
  }
  const model = newModel(thickness);
  model.root = pushPanel(model, {
    outline,
    holes: [],
    frameBent: identityFrame(),
    frameFlat: identityFrame(),
    incidentBends: [],
  });
  return model;
}

/**
 * Build a sheet-metal model from an axis-aligned rectangle in the XY plane.
 * Corner at origin, extending into +X and +Y by `(width, depth)`. Outside
 * face on +Z.
 */
export function baseFlangeRect(
  width: number,
  depth: number,
  thickness: number,
): SheetMetalModel {
  if (!(width > 0) || Number.isNaN(width)) {
    throw new BaseFlangeException({
      kind: "NonPositiveDimension",
      name: "width",
      value: width,
    });
  }
  if (!(depth > 0) || Number.isNaN(depth)) {
    throw new BaseFlangeException({
      kind: "NonPositiveDimension",
      name: "depth",
      value: depth,
    });
  }
  return baseFlangePolygon(
    [
      { x: 0, y: 0 },
      { x: width, y: 0 },
      { x: width, y: depth },
      { x: 0, y: depth },
    ],
    thickness,
  );
}
