/**
 * Translation from editor-session sketch constraints (`SketchConstraint`,
 * segment-index based) into persisted document-level `DesignConstraint`s
 * anchored at a sketch node — so constraints survive save/reload, re-solve
 * on parameter change, and certify as receipt claims instead of dying with
 * the editing session.
 */

import type {
  Anchor,
  DesignConstraint,
  EntityRef,
  NodeId,
  SketchConstraint,
} from "@vcad/ir";

function pointAnchor(node: NodeId, ref: EntityRef): Anchor | null {
  const seg = ref.index;
  switch (ref.type) {
    case "LineStart":
    case "ArcStart":
    case "Point": // free points don't exist in the app's segment model
      return { kind: "sketchPoint", node: Number(node), segment: seg, point: "start" };
    case "LineEnd":
    case "ArcEnd":
      return { kind: "sketchPoint", node: Number(node), segment: seg, point: "end" };
    case "Center":
      return { kind: "sketchPoint", node: Number(node), segment: seg, point: "center" };
    default:
      return null;
  }
}

function segmentAnchor(node: NodeId, segment: number): Anchor {
  return { kind: "sketchSegment", node: Number(node), segment };
}

/**
 * Translate one session constraint. Returns null for kinds the document
 * model doesn't carry (currently Radius — circles are render-only arcs).
 */
export function sketchConstraintToDesign(
  node: NodeId,
  c: SketchConstraint,
): DesignConstraint["kind"] | null {
  switch (c.type) {
    case "Coincident": {
      const a = pointAnchor(node, c.pointA);
      const b = pointAnchor(node, c.pointB);
      return a && b ? { type: "coincident", a, b } : null;
    }
    case "Horizontal":
      return {
        type: "horizontal",
        a: segmentAnchor(node, c.line),
        b: segmentAnchor(node, c.line),
      };
    case "Vertical":
      return {
        type: "vertical",
        a: segmentAnchor(node, c.line),
        b: segmentAnchor(node, c.line),
      };
    case "Parallel":
      return {
        type: "parallel",
        a: segmentAnchor(node, c.lineA),
        b: segmentAnchor(node, c.lineB),
      };
    case "Perpendicular":
      return {
        type: "perpendicular",
        a: segmentAnchor(node, c.lineA),
        b: segmentAnchor(node, c.lineB),
      };
    case "Fixed": {
      const a = pointAnchor(node, c.point);
      return a ? { type: "fixed", a } : null;
    }
    case "Distance": {
      const a = pointAnchor(node, c.pointA);
      const b = pointAnchor(node, c.pointB);
      return a && b ? { type: "distance", a, b, value: c.distance } : null;
    }
    case "Length":
      return { type: "length", a: segmentAnchor(node, c.line), value: c.length };
    case "EqualLength":
      return {
        type: "equalLength",
        a: segmentAnchor(node, c.lineA),
        b: segmentAnchor(node, c.lineB),
      };
    case "Angle":
      return {
        type: "angle",
        a: segmentAnchor(node, c.lineA),
        b: segmentAnchor(node, c.lineB),
        value: c.angleDeg,
      };
    case "Radius":
      return null; // no document-level radius kind yet
    default:
      return null;
  }
}

/** Does this design constraint reference the given sketch node? */
export function referencesSketchNode(c: DesignConstraint, node: NodeId): boolean {
  return Object.values(c.kind as unknown as Record<string, unknown>).some((v) => {
    const a = v as { kind?: string; node?: number };
    return (
      a != null &&
      typeof a === "object" &&
      (a.kind === "sketchPoint" || a.kind === "sketchSegment") &&
      Number(a.node) === Number(node)
    );
  });
}

/**
 * Merge a sketch's session constraints into a document constraint set:
 * existing constraints for that sketch node are replaced; everything else
 * is preserved. Ids continue the document's "cN" sequence.
 */
export function mergeSketchConstraints(
  existing: DesignConstraint[],
  node: NodeId,
  session: SketchConstraint[],
): DesignConstraint[] {
  const kept = existing.filter((c) => !referencesSketchNode(c, node));
  let max = 0;
  for (const c of kept) {
    const m = /^c(\d+)$/.exec(c.id);
    if (m) max = Math.max(max, Number(m[1]));
  }
  const translated: DesignConstraint[] = [];
  for (const sc of session) {
    const kind = sketchConstraintToDesign(node, sc);
    if (kind) {
      translated.push({ id: `c${++max}`, kind, driven: false });
    }
  }
  return [...kept, ...translated];
}
