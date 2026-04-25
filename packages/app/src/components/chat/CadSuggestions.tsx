import { useMemo } from "react";
import {
  useDocumentStore,
  useSketchStore,
  useUiStore,
  type PartInfo,
  type SelectionContext,
  type ToolbarTab,
} from "@vcad/core";
import type { Document, NodeId, Vec3 } from "@vcad/ir";
import { Suggestions, Suggestion } from "@/components/ai-elements/suggestion";

interface Props {
  selection: SelectionContext[];
  onPick: (text: string) => void;
}

// ---------------------------------------------------------------------------
// Part summary — pull the bits of geometry we need to size suggestions
// ---------------------------------------------------------------------------

interface PartSummary {
  name: string;
  kind: PartInfo["kind"];
  size?: Vec3;
  radius?: number;
  height?: number;
  filletRadius?: number;
  chamferDistance?: number;
  shellThickness?: number;
  patternCount?: number;
  patternSpacing?: number;
}

function nodeOp(doc: Document, nodeId: NodeId | undefined) {
  if (nodeId == null) return undefined;
  return doc.nodes[String(nodeId)]?.op;
}

function describePart(part: PartInfo, doc: Document): PartSummary {
  const summary: PartSummary = { name: part.name, kind: part.kind };
  switch (part.kind) {
    case "cube": {
      const op = nodeOp(doc, part.primitiveNodeId);
      if (op?.type === "Cube") summary.size = op.size;
      break;
    }
    case "cylinder": {
      const op = nodeOp(doc, part.primitiveNodeId);
      if (op?.type === "Cylinder") {
        summary.radius = op.radius;
        summary.height = op.height;
      }
      break;
    }
    case "sphere": {
      const op = nodeOp(doc, part.primitiveNodeId);
      if (op?.type === "Sphere") summary.radius = op.radius;
      break;
    }
    case "fillet": {
      const op = nodeOp(doc, part.filletNodeId);
      if (op?.type === "Fillet") summary.filletRadius = op.radius;
      break;
    }
    case "chamfer": {
      const op = nodeOp(doc, part.chamferNodeId);
      if (op?.type === "Chamfer") summary.chamferDistance = op.distance;
      break;
    }
    case "shell": {
      const op = nodeOp(doc, part.shellNodeId);
      if (op?.type === "Shell") summary.shellThickness = op.thickness;
      break;
    }
    case "linear-pattern": {
      const op = nodeOp(doc, part.patternNodeId);
      if (op?.type === "LinearPattern") {
        summary.patternCount = op.count;
        summary.patternSpacing = op.spacing;
      }
      break;
    }
    case "circular-pattern": {
      const op = nodeOp(doc, part.patternNodeId);
      if (op?.type === "CircularPattern") summary.patternCount = op.count;
      break;
    }
  }
  return summary;
}

// Round to a sensible engineering value so generated dims read like a human
// wrote them (5/10/25mm, not 4.7392mm).
function nice(n: number, min = 1): number {
  if (!Number.isFinite(n) || n <= 0) return min;
  if (n >= 100) return Math.round(n / 5) * 5;
  if (n >= 10) return Math.round(n);
  if (n >= 1) return Math.round(n * 2) / 2;
  return Math.max(min, Math.round(n * 10) / 10);
}

// ---------------------------------------------------------------------------
// Mode-specific suggestion sets
// ---------------------------------------------------------------------------

function suggestionsForSketch(): string[] {
  return [
    "Draw a 30mm circle at the origin",
    "Draw a 50×30mm rectangle",
    "Make these segments tangent",
    "Dimension the selected line 25mm",
    "Extrude 10mm",
    "Revolve 360° around X",
  ];
}

function suggestionsForAssembly(partsCount: number): string[] {
  if (partsCount < 2) {
    return [
      "Add another part to start the assembly",
      "Import a STEP file",
      "Make this the ground part",
    ];
  }
  return [
    "Add a revolute joint between the two selected parts",
    "Make the largest part the ground",
    "Run a 5-second physics simulation",
    "Drop everything from 100mm height",
  ];
}

function suggestionsForEmptyDoc(): string[] {
  return [
    "Create a 50mm cube",
    "Sketch a 30mm circle",
    "⌀20 × 40mm cylinder",
    "100×60×10mm plate, 5mm rounded corners",
    "Build a knurled knob",
  ];
}

function suggestionsForSceneNoSelection(partsCount: number): string[] {
  return [
    `What's the bounding box of the ${partsCount} parts?`,
    "Add a 5mm base plate under everything",
    "Center the scene on the origin",
    `Color all ${partsCount} parts brand red`,
    "Render a turntable from the front-top",
  ];
}

function suggestionsForMulti(summaries: PartSummary[]): string[] {
  const count = summaries.length;
  const names = summaries.map((s) => s.name);
  if (count === 2 && names[0] && names[1]) {
    return [
      `Subtract ${names[1]} from ${names[0]}`,
      `Union ${names[0]} and ${names[1]}`,
      `Intersect ${names[0]} with ${names[1]}`,
      `Align ${names[1]} on top of ${names[0]}`,
      "Mirror this pair across XZ",
    ];
  }
  return [
    `Union all ${count} parts`,
    `Linear pattern these along X, 50mm spacing`,
    `Distribute these ${count} evenly along X`,
    `Color these ${count} parts the same`,
  ];
}

function suggestionsForSingle(sel: SelectionContext, s: PartSummary): string[] {
  const name = s.name;

  // Sub-element selections (face/edge/vertex) — these hint at very specific
  // operations the AI can perform on the picked geometry.
  if (sel.geometryType === "face") {
    return [
      "Sketch on this face",
      "Extrude this face 10mm",
      "Fillet this face's edges 2mm",
      "Offset this face 1mm outward",
    ];
  }
  if (sel.geometryType === "edge") {
    return [
      "Fillet this edge 2mm",
      "Chamfer this edge 1mm",
      `Use this edge as a sketch line`,
    ];
  }
  if (sel.geometryType === "vertex") {
    return ["Fillet the edges meeting this vertex 1mm"];
  }

  switch (s.kind) {
    case "cube": {
      const sx = s.size?.x ?? 50;
      const sy = s.size?.y ?? 50;
      const sz = s.size?.z ?? 50;
      const minDim = Math.min(sx, sy, sz);
      const r = nice(minDim * 0.05, 1);
      const wall = nice(minDim * 0.05, 1);
      const hole = nice(minDim * 0.2, 2);
      return [
        `Fillet ${name} edges by ${r}mm`,
        `Chamfer the top of ${name} ${r}mm`,
        `Hollow ${name}, ${wall}mm wall`,
        `Drill a ⌀${hole}mm hole through ${name}`,
        `Mirror ${name} across XZ`,
      ];
    }
    case "cylinder": {
      const r = s.radius ?? 10;
      const h = s.height ?? 30;
      const wall = nice(r * 0.15, 1);
      const bore = nice(r * 0.5, 2);
      return [
        `Chamfer the ends of ${name} 1mm`,
        `Hollow ${name} into a ${wall}mm tube`,
        `Drill a ⌀${bore}mm axial bore through ${name}`,
        `Circular pattern ${name} around Z, 6 copies`,
        `Make ${name} ${nice(h * 1.5)}mm tall`,
      ];
    }
    case "sphere": {
      const r = s.radius ?? 25;
      const wall = nice(r * 0.1, 1);
      return [
        `Cut ${name} at Z=0 to make a hemisphere`,
        `Hollow ${name} to a ${wall}mm shell`,
        `Make ${name} ⌀${nice(r * 2 * 1.2)}mm`,
        `Pattern ${name} 4×4 grid, ${nice(r * 2.5)}mm apart`,
      ];
    }
    case "extrude":
      return [
        `Edit the sketch of ${name}`,
        `Change ${name}'s extrude depth`,
        `Add a 5° taper to ${name}`,
        `Twist ${name} 45° along its length`,
        `Fillet ${name}'s edges 1mm`,
      ];
    case "revolve":
      return [
        `Edit ${name}'s profile`,
        `Make ${name} a partial 180° revolve`,
        `Fillet ${name}'s edges 1mm`,
        `Mirror ${name} across XY`,
      ];
    case "sweep":
      return [
        `Edit ${name}'s profile`,
        `Add a twist to ${name}`,
        `Fillet ${name}'s edges 1mm`,
      ];
    case "loft":
      return [
        `Add another section to ${name}`,
        `Close ${name}'s ends`,
        `Fillet ${name}'s edges 1mm`,
      ];
    case "fillet": {
      const r = s.filletRadius ?? 2;
      return [
        `Increase ${name}'s radius to ${nice(r * 1.5)}mm`,
        `Decrease ${name}'s radius to ${nice(Math.max(0.5, r * 0.5))}mm`,
        `Apply ${name} to all edges`,
        `Convert ${name} to a chamfer`,
      ];
    }
    case "chamfer": {
      const d = s.chamferDistance ?? 1;
      return [
        `Increase ${name}'s distance to ${nice(d * 1.5)}mm`,
        `Convert ${name} to a fillet`,
        `Apply ${name} to all edges`,
      ];
    }
    case "shell": {
      const t = s.shellThickness ?? 2;
      return [
        `Change ${name}'s thickness to ${nice(t * 1.5)}mm`,
        `Pick different open faces on ${name}`,
        `Remove ${name}`,
      ];
    }
    case "linear-pattern":
    case "circular-pattern": {
      const c = s.patternCount ?? 4;
      return [
        `Increase ${name}'s count to ${c + 2}`,
        `Decrease ${name}'s count to ${Math.max(2, c - 1)}`,
        `Change ${name}'s spacing`,
        `Combine ${name} into a single solid`,
      ];
    }
    case "boolean":
      return [
        `Show the source parts of ${name}`,
        `Switch ${name} to a union`,
        `Fillet ${name}'s seams 1mm`,
        `Color ${name} brand red`,
      ];
    case "mirror":
      return [
        `Mirror ${name} across a different plane`,
        `Combine ${name} with its source`,
        `Remove ${name}`,
      ];
    case "imported-mesh":
      return [
        `Center ${name} on the origin`,
        `Scale ${name} to fit a 100mm cube`,
        `Mirror ${name} across XZ`,
        `Color ${name} aluminum`,
      ];
    case "text":
      return [
        `Change ${name}'s text`,
        `Make ${name} 50% bigger`,
        `Extrude ${name} 2mm`,
        `Mirror ${name} across XZ`,
      ];
    case "pcb-board":
      return [
        `Resize ${name} to 80×60mm`,
        `Add mounting holes to ${name}`,
        `Round ${name}'s corners 3mm`,
      ];
    case "embroidery-pattern":
    case "stitch":
      return [
        `Recolor ${name}`,
        `Scale ${name} to 50mm wide`,
        `Mirror ${name}`,
      ];
    default:
      return [
        `Fillet ${name} 2mm`,
        `Mirror ${name} across XZ`,
        `Make ${name} 20% larger`,
        `Color ${name} brand red`,
      ];
  }
}

// ---------------------------------------------------------------------------
// Top-level picker
// ---------------------------------------------------------------------------

interface Ctx {
  selection: SelectionContext[];
  partIndex: Map<string, PartInfo>;
  doc: Document;
  partsCount: number;
  sketchActive: boolean;
  toolbarTab: ToolbarTab;
}

function pickSuggestions(ctx: Ctx): string[] {
  if (ctx.sketchActive || ctx.toolbarTab === "sketch") {
    return suggestionsForSketch();
  }
  if (ctx.toolbarTab === "assembly") {
    return suggestionsForAssembly(ctx.partsCount);
  }
  if (ctx.selection.length === 0) {
    if (ctx.partsCount === 0) return suggestionsForEmptyDoc();
    return suggestionsForSceneNoSelection(ctx.partsCount);
  }

  const summaries = ctx.selection
    .map((sel) => ctx.partIndex.get(sel.partId))
    .filter((p): p is PartInfo => !!p)
    .map((p) => describePart(p, ctx.doc));

  if (ctx.selection.length === 1 && ctx.selection[0] && summaries[0]) {
    return suggestionsForSingle(ctx.selection[0], summaries[0]);
  }
  return suggestionsForMulti(summaries);
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function CadSuggestions({ selection, onPick }: Props) {
  const partIndex = useDocumentStore((s) => s.partIndex);
  const doc = useDocumentStore((s) => s.document);
  const partsCount = useDocumentStore((s) => s.parts.length);
  const sketchActive = useSketchStore((s) => s.active);
  const toolbarTab = useUiStore((s) => s.toolbarTab);

  const prompts = useMemo(
    () =>
      pickSuggestions({
        selection,
        partIndex,
        doc,
        partsCount,
        sketchActive,
        toolbarTab,
      }),
    [selection, partIndex, doc, partsCount, sketchActive, toolbarTab],
  );

  if (prompts.length === 0) return null;

  return (
    <Suggestions className="px-0">
      {prompts.map((p) => (
        <Suggestion
          key={p}
          suggestion={p}
          onClick={onPick}
          variant="ghost"
          className="h-6 shrink-0 rounded-full border border-border/40 bg-transparent px-2.5 text-[10px] font-normal text-text-muted shadow-none transition-colors hover:border-border hover:bg-hover hover:text-text"
        />
      ))}
    </Suggestions>
  );
}
