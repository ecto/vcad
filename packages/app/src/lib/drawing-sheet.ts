import {
  useEngineStore,
  useDocumentStore,
  type RenderedDimension,
  type SectionView,
  type OffsetSectionPlane,
  type TitleBlockFields,
  type BomRow,
  type DrawingSheetSpec,
} from "@vcad/core";
import type { DrawingSectionLine, DrawingTitleBlock } from "@vcad/ir";
import { useDrawingStore, type ViewDirection } from "@/stores/drawing-store";
import { downloadBlob } from "./download";

type Vec3 = [number, number, number];

const cross = (a: Vec3, b: Vec3): Vec3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const scale3 = (a: Vec3, s: number): Vec3 => [a[0] * s, a[1] * s, a[2] * s];
const normalize = (a: Vec3): Vec3 => {
  const n = Math.hypot(a[0], a[1], a[2]);
  return n > 1e-12 ? scale3(a, 1 / n) : a;
};

/** Orthographic view basis, mirroring the kernel's `ViewMatrix` construction
 * (`crates/vcad-kernel-drafting/src/projection.rs`): view x = p·right,
 * view y = p·up. Isometric views have no axis-aligned section semantics. */
export function viewBasis(
  dir: ViewDirection,
): { right: Vec3; up: Vec3; forward: Vec3 } | null {
  const table: Record<string, { forward: Vec3; worldUp: Vec3 }> = {
    front: { forward: [0, 1, 0], worldUp: [0, 0, 1] },
    back: { forward: [0, -1, 0], worldUp: [0, 0, 1] },
    top: { forward: [0, 0, -1], worldUp: [0, 1, 0] },
    bottom: { forward: [0, 0, 1], worldUp: [0, -1, 0] },
    right: { forward: [1, 0, 0], worldUp: [0, 0, 1] },
    left: { forward: [-1, 0, 0], worldUp: [0, 0, 1] },
  };
  const entry = table[dir];
  if (!entry) return null;
  const right = normalize(cross(entry.worldUp, entry.forward));
  const up = normalize(cross(entry.forward, right));
  return { right, up, forward: entry.forward };
}

/**
 * Derive an offset (stepped) section plane from a cut polyline drawn on an
 * orthographic view. The line runs vertically in view coordinates; each
 * roughly-vertical segment becomes one step, its horizontal position the
 * step's jog offset.
 */
export function sectionPlaneFromLine(
  dir: ViewDirection,
  points: Array<[number, number]>,
): OffsetSectionPlane | null {
  const basis = viewBasis(dir);
  if (!basis || points.length < 2) return null;

  const steps: Array<{ u_start: number; u_end: number; offset: number }> = [];
  let baseX: number | null = null;
  for (let i = 0; i < points.length - 1; i++) {
    const [x1, y1] = points[i]!;
    const [x2, y2] = points[i + 1]!;
    if (Math.abs(y2 - y1) <= Math.abs(x2 - x1)) continue; // jog segment
    const x = (x1 + x2) / 2;
    if (baseX === null) baseX = x;
    steps.push({
      u_start: Math.min(y1, y2),
      u_end: Math.max(y1, y2),
      offset: x - baseX,
    });
  }
  if (baseX === null) return null;

  // Cutting planes are perpendicular to the view's right axis; the section's
  // U axis (up × normal) must equal the view's up so step spans line up with
  // the drawn polyline's vertical extents.
  return {
    base: {
      origin: scale3(basis.right, baseX),
      normal: basis.right,
      up: basis.forward,
    },
    steps: steps.every((s) => s.offset === 0) ? [] : steps,
  };
}

/** Concatenate all scene part meshes into a single mesh for sectioning. */
export function combineSceneMeshes(): {
  positions: Float32Array;
  indices: Uint32Array;
} | null {
  const scene = useEngineStore.getState().scene;
  if (!scene?.parts?.length) return null;

  let vertCount = 0;
  let idxCount = 0;
  for (const part of scene.parts) {
    vertCount += part.mesh.positions.length;
    idxCount += part.mesh.indices.length;
  }
  const positions = new Float32Array(vertCount);
  const indices = new Uint32Array(idxCount);
  let vOff = 0;
  let iOff = 0;
  for (const part of scene.parts) {
    positions.set(part.mesh.positions, vOff);
    const base = vOff / 3;
    for (let i = 0; i < part.mesh.indices.length; i++) {
      indices[iOff + i] = part.mesh.indices[i]! + base;
    }
    vOff += part.mesh.positions.length;
    iOff += part.mesh.indices.length;
  }
  return { positions, indices };
}

/** Compute the section view for a persisted section line. */
export function computeSectionView(section: DrawingSectionLine): SectionView | null {
  const engine = useEngineStore.getState().engine;
  if (!engine) return null;
  const mesh = combineSceneMeshes();
  if (!mesh) return null;
  const plane = sectionPlaneFromLine(
    section.view as ViewDirection,
    section.points as Array<[number, number]>,
  );
  if (!plane) return null;
  return engine.offsetSectionMesh(mesh, plane, { spacing: 3, angle: Math.PI / 4 });
}

/** Kernel-shaped title block fields from the persisted (camelCase) IR shape. */
export function toKernelTitleBlock(tb: DrawingTitleBlock, scaleNote: string): TitleBlockFields {
  return {
    part_name: tb.partName || "UNTITLED",
    material: tb.material || "",
    finish: "",
    scale: tb.scale || scaleNote,
    drawn_by: tb.author || "",
    date: tb.date || "",
    revision: tb.revision || "A",
    units: "MM",
    tolerance_note: "±0.1 UNLESS NOTED",
  };
}

/** BOM rows from the document's part list. */
export function buildBomRows(): BomRow[] {
  const state = useDocumentStore.getState();
  const materials = state.document.part_materials ?? {};
  return state.parts.map((part, i) => ({
    item: i + 1,
    name: part.name || part.id,
    qty: 1,
    material: materials[part.name] ?? materials[part.id] ?? "",
  }));
}

/** Map a rendered dimension from view coordinates to sheet coordinates:
 * scale about `viewCenter`, translate so it lands at `sheetCenter`. Text and
 * arrow sizes stay in sheet mm (readable at any view scale). */
function dimensionToSheet(
  rd: RenderedDimension,
  viewCenter: [number, number],
  sheetCenter: [number, number],
  s: number,
): RenderedDimension {
  const map = (p: { x: number; y: number }) => ({
    x: sheetCenter[0] + (p.x - viewCenter[0]) * s,
    y: sheetCenter[1] + (p.y - viewCenter[1]) * s,
  });
  return {
    lines: rd.lines.map(([a, b]) => [map(a), map(b)]),
    arcs: rd.arcs.map((a) => ({ ...a, center: map(a.center), radius: a.radius * s })),
    arrows: rd.arrows.map((a) => ({ ...a, tip: map(a.tip) })),
    texts: rd.texts.map((t) => ({ ...t, position: map(t.position) })),
    is_basic: rd.is_basic,
  };
}

/** A cut-line annotation (polyline + end labels) in view coordinates. */
function cutLineDimension(section: DrawingSectionLine): RenderedDimension {
  const pts = section.points as Array<[number, number]>;
  const lines: RenderedDimension["lines"] = [];
  for (let i = 0; i < pts.length - 1; i++) {
    lines.push([
      { x: pts[i]![0], y: pts[i]![1] },
      { x: pts[i + 1]![0], y: pts[i + 1]![1] },
    ]);
  }
  const mkText = (p: [number, number]) => ({
    position: { x: p[0], y: p[1] },
    text: section.label,
    height: 4,
    rotation: 0,
    alignment: "MiddleCenter",
  });
  return {
    lines,
    arcs: [],
    arrows: [],
    texts: pts.length ? [mkText(pts[0]!), mkText(pts[pts.length - 1]!)] : [],
    is_basic: false,
  };
}

/** Round a drawing scale to a conventional value (1:1, 1:2, 2:1, …). */
function niceScale(s: number): { value: number; note: string } {
  const standards = [100, 50, 20, 10, 5, 2, 1, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01];
  const value = standards.find((v) => v <= s) ?? standards[standards.length - 1]!;
  const note = value >= 1 ? `${value}:1` : `1:${Math.round(1 / value)}`;
  return { value, note };
}

/**
 * Compose the shop-drawing sheet spec from current app state: the active
 * orthographic view, its dimensions, section views for this view's cut
 * lines, the persisted title block, and the BOM when enabled.
 */
export function buildDrawingSheetSpec(): DrawingSheetSpec | null {
  const engine = useEngineStore.getState().engine;
  const scene = useEngineStore.getState().scene;
  if (!engine || !scene?.parts?.length) return null;

  const { viewDirection } = useDrawingStore.getState();
  const doc = useDocumentStore.getState().document;
  const drawing = doc.drawing;

  const mesh = combineSceneMeshes();
  if (!mesh) return null;
  const view = engine.projectMesh(mesh, viewDirection);
  if (!view) return null;

  const sections = (drawing?.sections ?? []).filter((s) => s.view === viewDirection);
  const showBom = drawing?.showBom ?? false;

  // Sheet layout: A4 landscape (297×210), 10 mm margin. Reserve the right
  // strip for sections and the bottom-right for title block + BOM.
  const hasSections = sections.length > 0;
  const mainBoxW = hasSections ? 140 : 200;
  const mainBoxH = 130;
  const mainCenter: [number, number] = hasSections ? [85, 120] : [120, 120];

  const vw = view.bounds.max_x - view.bounds.min_x || 1;
  const vh = view.bounds.max_y - view.bounds.min_y || 1;
  const { value: s, note: scaleNote } = niceScale(
    Math.min(mainBoxW / vw, mainBoxH / vh),
  );
  const viewCenter: [number, number] = [
    (view.bounds.min_x + view.bounds.max_x) / 2,
    (view.bounds.min_y + view.bounds.max_y) / 2,
  ];

  const spec: DrawingSheetSpec = {
    size: "a4",
    views: [
      {
        view,
        center: mainCenter,
        scale: s,
        label: `${viewDirection.toUpperCase()} VIEW  (${scaleNote})`,
      },
    ],
    sections: [],
    annotations: [],
  };

  // Overall dimensions, transformed into sheet coordinates.
  const AnnotationLayer = engine.WasmAnnotationLayer;
  try {
    const layer = new AnnotationLayer();
    layer.addHorizontalDimension(
      view.bounds.min_x, view.bounds.min_y, view.bounds.max_x, view.bounds.min_y, -10 / s,
    );
    layer.addVerticalDimension(
      view.bounds.max_x, view.bounds.min_y, view.bounds.max_x, view.bounds.max_y, 10 / s,
    );
    const rendered = layer.renderAll() as RenderedDimension[];
    layer.free();
    for (const rd of rendered) {
      spec.annotations!.push(dimensionToSheet(rd, viewCenter, mainCenter, s));
    }
  } catch {
    // Dimensions are best-effort; the sheet is still valid without them.
  }

  // Section views stacked in the right strip, plus cut lines on the main view.
  const sectionCenterX = 225;
  let sectionY = 160;
  for (const section of sections) {
    const sv = computeSectionView(section);
    if (!sv) continue;
    spec.sections!.push({
      view: sv,
      center: [sectionCenterX, sectionY],
      scale: s,
      label: `SECTION ${section.label}-${section.label}  (${scaleNote})`,
    });
    sectionY -= 65;
    spec.annotations!.push(
      dimensionToSheet(cutLineDimension(section), viewCenter, mainCenter, s),
    );
  }

  if (drawing?.titleBlock) {
    spec.title_block = toKernelTitleBlock(drawing.titleBlock, scaleNote);
  }
  if (showBom) {
    const rows = buildBomRows();
    if (rows.length > 0) spec.bom = rows;
  }

  return spec;
}

/** Export the current drawing as a kernel-rendered PDF. Returns an error
 * message on failure, null on success. */
export function exportDrawingPdf(): string | null {
  const engine = useEngineStore.getState().engine;
  if (!engine) return "Engine not ready";
  const spec = buildDrawingSheetSpec();
  if (!spec) return "Nothing to export";
  let bytes: Uint8Array | null;
  try {
    bytes = engine.drawingSheetToPdf(spec);
  } catch (err) {
    return `PDF export failed: ${(err as Error).message}`;
  }
  if (!bytes) return "Kernel build does not support PDF export";
  const { viewDirection } = useDrawingStore.getState();
  const blob = new Blob([new Uint8Array(bytes)], { type: "application/pdf" });
  downloadBlob(blob, `drawing-${viewDirection}.pdf`);
  return null;
}
