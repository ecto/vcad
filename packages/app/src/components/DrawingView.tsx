import { useMemo, useRef, useEffect, useCallback, useState } from "react";
import {
  useEngineStore,
  useDocumentStore,
  useUiStore,
  type ProjectedView,
  type RenderedDimension,
  type DetailView,
  type SectionView,
  type RenderedBlock,
} from "@vcad/core";
import type { DrawingSectionLine } from "@vcad/ir";
import { useDrawingStore, type DetailViewDef, type ViewDirection } from "../stores/drawing-store";
import {
  sectionPlaneFromLine,
  combineSceneMeshes,
  toKernelTitleBlock,
  buildBomRows,
} from "@/lib/drawing-sheet";
import { useTheme } from "@/hooks/useTheme";

// Color schemes for light and dark modes
const COLORS = {
  light: {
    background: "#ffffff",
    edge: "#1a1a1a",
    hiddenEdge: "#999999",
    dimension: "#3b82f6",
    label: "#666666",
    selection: "#3b82f6",
    selectionFill: "rgba(59, 130, 246, 0.1)",
    detailRegion: "#e11d48",
    detailRegionFill: "rgba(225, 29, 72, 0.05)",
    section: "#d97706",
    sheet: "#444444",
  },
  dark: {
    background: "#0a0a0a",
    edge: "#00d4aa",
    hiddenEdge: "#006655",
    dimension: "#ff6b35",
    label: "#888888",
    selection: "#00d4aa",
    selectionFill: "rgba(0, 212, 170, 0.1)",
    detailRegion: "#f43f5e",
    detailRegionFill: "rgba(244, 63, 94, 0.08)",
    section: "#fbbf24",
    sheet: "#aaaaaa",
  },
};

/** Render a kernel-drawn block (title block / BOM table) anchored with its
 * bottom-left corner at (`ax`, `aySvg`) in SVG coordinates, scaled by `sb`. */
function renderBlock(
  block: RenderedBlock,
  ax: number,
  aySvg: number,
  sb: number,
  color: string,
  strokeWidth: number,
  keyPrefix: string,
) {
  const mx = (x: number) => ax + x * sb;
  const my = (y: number) => aySvg - y * sb;
  return (
    <g key={keyPrefix} pointerEvents="none">
      {block.rendered.lines.map(([a, b], i) => (
        <line
          key={`${keyPrefix}-l-${i}`}
          x1={mx(a.x)}
          y1={my(a.y)}
          x2={mx(b.x)}
          y2={my(b.y)}
          stroke={color}
          strokeWidth={strokeWidth}
        />
      ))}
      {block.rendered.texts.map((t, i) => {
        const alignment = String(t.alignment);
        const anchor = alignment.includes("Left")
          ? "start"
          : alignment.includes("Right")
            ? "end"
            : "middle";
        const baseline = alignment.startsWith("Top")
          ? "hanging"
          : alignment.startsWith("Bottom")
            ? "auto"
            : "middle";
        return (
          <text
            key={`${keyPrefix}-t-${i}`}
            x={mx(t.position.x)}
            y={my(t.position.y)}
            fontSize={t.height * sb}
            fill={color}
            textAnchor={anchor}
            dominantBaseline={baseline}
            fontFamily="monospace"
          >
            {t.text}
          </text>
        );
      })}
    </g>
  );
}

/**
 * SVG-based 2D technical drawing view.
 *
 * Features:
 * - Visible edges as solid lines
 * - Hidden edges as dashed lines (optional)
 * - Auto-generated dimension annotations
 * - Dark mode support
 * - Zoom (scroll wheel / pinch)
 * - Pan (drag / two-finger scroll)
 * - Click to select parts
 */
export function DrawingView() {
  const {
    viewDirection,
    showHiddenLines,
    showDimensions,
    zoom,
    pan,
    adjustZoom,
    adjustPan,
    resetView,
    detailViews,
    addDetailView,
  } = useDrawingStore();
  const scene = useEngineStore((s) => s.scene);
  const engine = useEngineStore((s) => s.engine);
  const parts = useDocumentStore((s) => s.parts);
  const drawing = useDocumentStore((s) => s.document.drawing);
  const setDrawingSettings = useDocumentStore((s) => s.setDrawingSettings);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const select = useUiStore((s) => s.select);
  const clearSelection = useUiStore((s) => s.clearSelection);
  const { isDark } = useTheme();

  const svgRef = useRef<SVGSVGElement>(null);
  const [isPanning, setIsPanning] = useState(false);
  const [lastPointer, setLastPointer] = useState({ x: 0, y: 0 });

  // Momentum-based pan (like 3D orbit)
  const velocityRef = useRef({ x: 0, y: 0 });
  const animatingRef = useRef(false);

  // Detail view creation state
  const [isCreatingDetail, setIsCreatingDetail] = useState(false);
  const [detailStart, setDetailStart] = useState<{ x: number; y: number } | null>(null);
  const [detailEnd, setDetailEnd] = useState<{ x: number; y: number } | null>(null);

  // Section line creation state
  const [isCreatingSection, setIsCreatingSection] = useState(false);
  const [sectionPoints, setSectionPoints] = useState<Array<[number, number]>>([]);
  const [sectionCursor, setSectionCursor] = useState<{ x: number; y: number } | null>(null);

  const colors = isDark ? COLORS.dark : COLORS.light;

  // Project all parts' meshes to 2D view
  const projectedViews = useMemo<Array<{ partId: string; view: ProjectedView }>>(() => {
    if (!scene?.parts || !engine) return [];

    return scene.parts
      .map((evalPart, idx) => {
        const partInfo = parts[idx];
        if (!partInfo) return null;

        const view = engine.projectMesh(evalPart.mesh, viewDirection);
        if (!view) return null;

        return { partId: partInfo.id, view };
      })
      .filter((v): v is { partId: string; view: ProjectedView } => v !== null);
  }, [scene, engine, viewDirection, parts]);

  // Combined bounds of all parts
  const combinedBounds = useMemo(() => {
    if (projectedViews.length === 0) return null;

    let min_x = Infinity, min_y = Infinity, max_x = -Infinity, max_y = -Infinity;

    for (const { view } of projectedViews) {
      min_x = Math.min(min_x, view.bounds.min_x);
      min_y = Math.min(min_y, view.bounds.min_y);
      max_x = Math.max(max_x, view.bounds.max_x);
      max_y = Math.max(max_y, view.bounds.max_y);
    }

    return { min_x, min_y, max_x, max_y };
  }, [projectedViews]);

  // Combine all projected views into one for detail view creation
  const combinedView = useMemo<ProjectedView | null>(() => {
    if (projectedViews.length === 0) return null;

    // Combine all edges from all parts into a single view
    const allEdges = projectedViews.flatMap((pv) => pv.view.edges);
    return {
      edges: allEdges,
      bounds: combinedBounds!,
      view_direction: viewDirection,
    };
  }, [projectedViews, combinedBounds, viewDirection]);

  // Generate detail views from definitions
  const computedDetailViews = useMemo<Array<{ def: DetailViewDef; view: DetailView }>>(() => {
    if (!combinedView || !engine || detailViews.length === 0) return [];

    return detailViews
      .map((def) => {
        try {
          const view = engine.createDetailView(
            combinedView,
            def.centerX,
            def.centerY,
            def.scale,
            def.width,
            def.height,
            def.label
          );
          return { def, view };
        } catch {
          return null;
        }
      })
      .filter((v): v is { def: DetailViewDef; view: DetailView } => v !== null);
  }, [combinedView, engine, detailViews]);

  // Section cut lines for the current view + their computed section views
  const computedSections = useMemo<
    Array<{ def: DrawingSectionLine; view: SectionView }>
  >(() => {
    if (!engine || !scene?.parts?.length) return [];
    const defs = (drawing?.sections ?? []).filter((s) => s.view === viewDirection);
    if (defs.length === 0) return [];
    const mesh = combineSceneMeshes();
    if (!mesh) return [];

    return defs
      .map((def) => {
        const plane = sectionPlaneFromLine(
          def.view as ViewDirection,
          def.points as Array<[number, number]>,
        );
        if (!plane) return null;
        const view = engine.offsetSectionMesh(mesh, plane, {
          spacing: 3,
          angle: Math.PI / 4,
        });
        return view ? { def, view } : null;
      })
      .filter((v): v is { def: DrawingSectionLine; view: SectionView } => v !== null);
  }, [engine, scene, drawing, viewDirection]);

  // Kernel-rendered title block (matches the exported PDF)
  const titleBlock = useMemo<RenderedBlock | null>(() => {
    if (!engine || !drawing?.titleBlock) return null;
    try {
      return engine.renderTitleBlock(toKernelTitleBlock(drawing.titleBlock, ""));
    } catch {
      return null;
    }
  }, [engine, drawing]);

  // Kernel-rendered BOM table for assembly drawings
  const bomBlock = useMemo<RenderedBlock | null>(() => {
    if (!engine || !drawing?.showBom || parts.length === 0) return null;
    try {
      const rows = buildBomRows();
      return rows.length > 0 ? engine.renderBomTable(rows) : null;
    } catch {
      return null;
    }
  }, [engine, drawing, parts]);

  // Generate dimension annotations from combined bounding box
  const dimensions = useMemo<RenderedDimension[]>(() => {
    if (!combinedBounds || !showDimensions || !engine) return [];

    try {
      const WasmAnnotationLayer = engine.WasmAnnotationLayer;
      const annotations = new WasmAnnotationLayer();
      const { min_x, min_y, max_x, max_y } = combinedBounds;

      // Width dimension at bottom
      const bottomOffset = -10;
      annotations.addHorizontalDimension(min_x, min_y, max_x, min_y, bottomOffset);

      // Height dimension at right
      const rightOffset = 10;
      annotations.addVerticalDimension(max_x, min_y, max_x, max_y, rightOffset);

      const rendered = annotations.renderAll();
      annotations.free();
      return rendered as RenderedDimension[];
    } catch {
      return [];
    }
  }, [combinedBounds, showDimensions, engine]);

  // Handle wheel for pan/zoom with momentum
  const handleWheel = useCallback(
    (e: WheelEvent) => {
      e.preventDefault();

      // Shift+scroll = zoom
      if (e.shiftKey) {
        const delta = -e.deltaY * 0.002;
        adjustZoom(delta);
        return;
      }

      // Normalize deltaMode: 0=pixels, 1=lines, 2=pages
      let dx = e.deltaX;
      let dy = e.deltaY;
      if (e.deltaMode === 1) {
        dx *= 16;
        dy *= 16;
      }
      if (e.deltaMode === 2) {
        dx *= 100;
        dy *= 100;
      }

      // Reverse direction: drag right → view moves right (content moves left)
      // Scale by zoom for consistent feel at any zoom level
      const panScale = 0.15 / zoom;
      velocityRef.current.x -= dx * panScale;
      velocityRef.current.y -= dy * panScale;

      // Start animation loop if not already running
      if (!animatingRef.current) {
        animatingRef.current = true;
        const animate = () => {
          const vel = velocityRef.current;

          // Stop when velocity is negligible
          if (Math.abs(vel.x) < 0.01 && Math.abs(vel.y) < 0.01) {
            animatingRef.current = false;
            vel.x = 0;
            vel.y = 0;
            return;
          }

          // Apply fraction of velocity
          const dampingFactor = 0.15;
          adjustPan(vel.x * dampingFactor, vel.y * dampingFactor);

          // Decay velocity (friction)
          const friction = 0.92;
          vel.x *= friction;
          vel.y *= friction;

          requestAnimationFrame(animate);
        };
        requestAnimationFrame(animate);
      }
    },
    [adjustZoom, adjustPan, zoom]
  );

  // Attach wheel listener
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;

    svg.addEventListener("wheel", handleWheel, { passive: false });
    return () => svg.removeEventListener("wheel", handleWheel);
  }, [handleWheel]);

  // Handle pointer down for pan start
  const handlePointerDown = useCallback((e: React.PointerEvent) => {
    // Middle mouse button or alt+click for pan
    if (e.button === 1 || (e.button === 0 && e.altKey)) {
      setIsPanning(true);
      setLastPointer({ x: e.clientX, y: e.clientY });
      // Stop any ongoing momentum animation when starting a new drag
      velocityRef.current = { x: 0, y: 0 };
      animatingRef.current = false;
      (e.target as Element).setPointerCapture(e.pointerId);
      e.preventDefault();
    }
  }, []);

  // Handle pointer move for panning
  const handlePointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (!isPanning) return;

      const dx = e.clientX - lastPointer.x;
      const dy = e.clientY - lastPointer.y;
      setLastPointer({ x: e.clientX, y: e.clientY });

      // Convert screen pixels to SVG units
      const svg = svgRef.current;
      if (!svg) return;

      const rect = svg.getBoundingClientRect();
      const viewBoxWidth = combinedBounds
        ? (combinedBounds.max_x - combinedBounds.min_x) * 1.6
        : 100;
      const scale = viewBoxWidth / rect.width / zoom;

      // Direct pan while dragging
      const panX = dx * scale;
      const panY = -dy * scale;
      adjustPan(panX, panY);

      // Track velocity for momentum on release
      velocityRef.current.x = panX;
      velocityRef.current.y = panY;
    },
    [isPanning, lastPointer, adjustPan, zoom, combinedBounds]
  );

  // Handle pointer up to end pan (with momentum)
  const handlePointerUp = useCallback((e: React.PointerEvent) => {
    if (isPanning) {
      setIsPanning(false);
      (e.target as Element).releasePointerCapture(e.pointerId);

      // Start momentum animation if there's velocity
      const vel = velocityRef.current;
      if (Math.abs(vel.x) > 0.1 || Math.abs(vel.y) > 0.1) {
        if (!animatingRef.current) {
          animatingRef.current = true;
          const animate = () => {
            // Stop when velocity is negligible
            if (Math.abs(vel.x) < 0.01 && Math.abs(vel.y) < 0.01) {
              animatingRef.current = false;
              vel.x = 0;
              vel.y = 0;
              return;
            }

            // Apply fraction of velocity
            const dampingFactor = 0.15;
            useDrawingStore.getState().adjustPan(vel.x * dampingFactor, vel.y * dampingFactor);

            // Decay velocity (friction)
            const friction = 0.92;
            vel.x *= friction;
            vel.y *= friction;

            requestAnimationFrame(animate);
          };
          requestAnimationFrame(animate);
        }
      }
    }
  }, [isPanning]);

  // Handle click to select part
  const handlePartClick = useCallback(
    (partId: string, e: React.MouseEvent) => {
      e.stopPropagation();
      if (e.shiftKey) {
        // Multi-select with shift
        if (selectedPartIds.has(partId)) {
          // Deselect
          const newSelection = new Set(selectedPartIds);
          newSelection.delete(partId);
          useUiStore.getState().selectMultiple(Array.from(newSelection));
        } else {
          useUiStore.getState().selectMultiple([...Array.from(selectedPartIds), partId]);
        }
      } else {
        select(partId);
      }
    },
    [selectedPartIds, select]
  );

  // Handle background click to clear selection
  const handleBackgroundClick = useCallback(() => {
    clearSelection();
  }, [clearSelection]);

  // Handle double-click to reset view
  const handleDoubleClick = useCallback(() => {
    resetView();
  }, [resetView]);

  // Convert screen coordinates to SVG coordinates
  const screenToSvg = useCallback(
    (clientX: number, clientY: number): { x: number; y: number } | null => {
      const svg = svgRef.current;
      if (!svg || !combinedBounds) return null;

      const rect = svg.getBoundingClientRect();
      const width = combinedBounds.max_x - combinedBounds.min_x;
      const height = combinedBounds.max_y - combinedBounds.min_y;
      const padding = Math.max(width, height) * 0.3;
      const viewWidth = (width + 2 * padding) / zoom;
      const viewHeight = (height + 2 * padding) / zoom;
      const viewX = combinedBounds.min_x - padding + (width + 2 * padding - viewWidth) / 2 - pan.x;
      const viewY = -combinedBounds.max_y - padding + (height + 2 * padding - viewHeight) / 2 - pan.y;

      // Map client coords to viewBox
      const svgX = viewX + ((clientX - rect.left) / rect.width) * viewWidth;
      const svgY = viewY + ((clientY - rect.top) / rect.height) * viewHeight;

      return { x: svgX, y: -svgY }; // Flip Y back to normal coords
    },
    [combinedBounds, zoom, pan]
  );

  // Start creating a detail view
  const startDetailCreation = useCallback(() => {
    setIsCreatingDetail(true);
    setDetailStart(null);
    setDetailEnd(null);
  }, []);

  // Handle mouse down for detail region selection
  const handleDetailMouseDown = useCallback(
    (e: React.MouseEvent) => {
      if (!isCreatingDetail) return;

      const pos = screenToSvg(e.clientX, e.clientY);
      if (pos) {
        setDetailStart(pos);
        setDetailEnd(pos);
      }
    },
    [isCreatingDetail, screenToSvg]
  );

  // Handle mouse move for detail region selection
  const handleDetailMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (!isCreatingDetail || !detailStart) return;

      const pos = screenToSvg(e.clientX, e.clientY);
      if (pos) {
        setDetailEnd(pos);
      }
    },
    [isCreatingDetail, detailStart, screenToSvg]
  );

  // Handle mouse up to finish detail region selection
  const handleDetailMouseUp = useCallback(() => {
    if (!isCreatingDetail || !detailStart || !detailEnd) return;

    const minX = Math.min(detailStart.x, detailEnd.x);
    const maxX = Math.max(detailStart.x, detailEnd.x);
    const minY = Math.min(detailStart.y, detailEnd.y);
    const maxY = Math.max(detailStart.y, detailEnd.y);

    const width = maxX - minX;
    const height = maxY - minY;

    // Only create if the region has some size
    if (width > 0.5 && height > 0.5) {
      const centerX = (minX + maxX) / 2;
      const centerY = (minY + maxY) / 2;

      // Auto-generate label (A, B, C, ...)
      const labelIndex = detailViews.length;
      const label = String.fromCharCode(65 + (labelIndex % 26));

      addDetailView({
        centerX,
        centerY,
        scale: 2.0, // Default 2x magnification
        width,
        height,
        label,
      });
    }

    setIsCreatingDetail(false);
    setDetailStart(null);
    setDetailEnd(null);
  }, [isCreatingDetail, detailStart, detailEnd, detailViews.length, addDetailView]);

  // Cancel detail creation on escape
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && isCreatingDetail) {
        setIsCreatingDetail(false);
        setDetailStart(null);
        setDetailEnd(null);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isCreatingDetail]);

  // Listen for start-detail-view event from toolbar
  useEffect(() => {
    const handleStartDetailView = () => {
      startDetailCreation();
    };
    window.addEventListener("vcad:start-detail-view", handleStartDetailView);
    return () => window.removeEventListener("vcad:start-detail-view", handleStartDetailView);
  }, [startDetailCreation]);

  // Listen for start-section-view event from toolbar
  useEffect(() => {
    const handleStartSection = () => {
      setIsCreatingSection(true);
      setSectionPoints([]);
      setSectionCursor(null);
    };
    window.addEventListener("vcad:start-section-view", handleStartSection);
    return () => window.removeEventListener("vcad:start-section-view", handleStartSection);
  }, []);

  // Commit the in-progress section line to the document
  const commitSection = useCallback(() => {
    if (sectionPoints.length >= 2) {
      const existing = drawing?.sections ?? [];
      const label = String.fromCharCode(65 + (existing.length % 26));
      setDrawingSettings({
        sections: [
          ...existing,
          {
            id: `section-${label}-${Date.now()}`,
            view: viewDirection,
            label,
            points: sectionPoints,
          },
        ],
      });
    }
    setIsCreatingSection(false);
    setSectionPoints([]);
    setSectionCursor(null);
  }, [sectionPoints, drawing, setDrawingSettings, viewDirection]);

  // Section creation: click adds a point, Enter commits, Escape cancels
  const handleSectionClick = useCallback(
    (e: React.MouseEvent) => {
      const pos = screenToSvg(e.clientX, e.clientY);
      if (pos) setSectionPoints((pts) => [...pts, [pos.x, pos.y]]);
    },
    [screenToSvg],
  );

  const handleSectionMouseMove = useCallback(
    (e: React.MouseEvent) => {
      const pos = screenToSvg(e.clientX, e.clientY);
      if (pos) setSectionCursor(pos);
    },
    [screenToSvg],
  );

  useEffect(() => {
    if (!isCreatingSection) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setIsCreatingSection(false);
        setSectionPoints([]);
        setSectionCursor(null);
      } else if (e.key === "Enter") {
        commitSection();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isCreatingSection, commitSection]);

  if (!combinedBounds || projectedViews.length === 0) {
    return (
      <div
        className="flex h-full w-full items-center justify-center"
        style={{ backgroundColor: colors.background, color: colors.label }}
      >
        No geometry to display
      </div>
    );
  }

  const width = combinedBounds.max_x - combinedBounds.min_x;
  const height = combinedBounds.max_y - combinedBounds.min_y;
  const padding = Math.max(width, height) * 0.3;

  // Apply pan and zoom to viewBox
  const viewWidth = (width + 2 * padding) / zoom;
  const viewHeight = (height + 2 * padding) / zoom;
  const viewX = combinedBounds.min_x - padding + (width + 2 * padding - viewWidth) / 2 - pan.x;
  const viewY = -combinedBounds.max_y - padding + (height + 2 * padding - viewHeight) / 2 - pan.y;

  const viewBox = `${viewX} ${viewY} ${viewWidth} ${viewHeight}`;

  // Line widths scaled to view (adjusted for zoom)
  const baseStroke = Math.max(width, height) * 0.005;
  const strokeWidth = baseStroke / Math.sqrt(zoom);
  const hiddenStrokeWidth = strokeWidth * 0.6;
  const dimStrokeWidth = strokeWidth * 0.4;

  // Sheet furniture layout (title block bottom-right, BOM above it)
  const sheetPad = viewWidth * 0.015;
  const tbScale = titleBlock ? (viewWidth * 0.3) / titleBlock.width : 0;
  const tbHeightSvg = titleBlock ? titleBlock.height * tbScale : 0;
  const tbClearance = titleBlock ? tbHeightSvg + sheetPad : 0;
  const bomScale = bomBlock ? (viewWidth * 0.3) / bomBlock.width : 0;

  return (
    <svg
      ref={svgRef}
      className="h-full w-full select-none"
      viewBox={viewBox}
      preserveAspectRatio="xMidYMid meet"
      style={{
        backgroundColor: colors.background,
        cursor:
          isCreatingDetail || isCreatingSection
            ? "crosshair"
            : isPanning
              ? "grabbing"
              : "default",
      }}
      onPointerDown={isCreatingDetail || isCreatingSection ? undefined : handlePointerDown}
      onPointerMove={isCreatingDetail || isCreatingSection ? undefined : handlePointerMove}
      onPointerUp={isCreatingDetail || isCreatingSection ? undefined : handlePointerUp}
      onPointerLeave={isCreatingDetail || isCreatingSection ? undefined : handlePointerUp}
      onMouseDown={isCreatingDetail ? handleDetailMouseDown : undefined}
      onMouseMove={
        isCreatingDetail
          ? handleDetailMouseMove
          : isCreatingSection
            ? handleSectionMouseMove
            : undefined
      }
      onMouseUp={isCreatingDetail ? handleDetailMouseUp : undefined}
      onClick={
        isCreatingDetail
          ? undefined
          : isCreatingSection
            ? handleSectionClick
            : handleBackgroundClick
      }
      onDoubleClick={isCreatingSection ? commitSection : handleDoubleClick}
    >
      {/* Grid pattern for dark mode */}
      {isDark && (
        <defs>
          <pattern
            id="grid"
            width={strokeWidth * 20}
            height={strokeWidth * 20}
            patternUnits="userSpaceOnUse"
          >
            <path
              d={`M ${strokeWidth * 20} 0 L 0 0 0 ${strokeWidth * 20}`}
              fill="none"
              stroke="#1a1a1a"
              strokeWidth={strokeWidth * 0.2}
            />
          </pattern>
        </defs>
      )}
      {isDark && (
        <rect
          x={viewX}
          y={viewY}
          width={viewWidth}
          height={viewHeight}
          fill="url(#grid)"
        />
      )}

      {/* Render each part's edges */}
      {projectedViews.map(({ partId, view }) => {
        const isSelected = selectedPartIds.has(partId);

        return (
          <g
            key={partId}
            onClick={(e) => handlePartClick(partId, e)}
            style={{ cursor: "pointer" }}
          >
            {/* Selection highlight */}
            {isSelected && (
              <g>
                {view.edges
                  .filter((e) => e.visibility === "Visible")
                  .map((edge, i) => (
                    <line
                      key={`sel-${i}`}
                      x1={edge.start.x}
                      y1={-edge.start.y}
                      x2={edge.end.x}
                      y2={-edge.end.y}
                      stroke={colors.selection}
                      strokeWidth={strokeWidth * 3}
                      strokeLinecap="round"
                      opacity={0.3}
                    />
                  ))}
              </g>
            )}

            {/* Visible edges */}
            {view.edges
              .filter((e) => e.visibility === "Visible")
              .map((edge, i) => (
                <line
                  key={`v-${i}`}
                  x1={edge.start.x}
                  y1={-edge.start.y}
                  x2={edge.end.x}
                  y2={-edge.end.y}
                  stroke={isSelected ? colors.selection : colors.edge}
                  strokeWidth={strokeWidth}
                  strokeLinecap="round"
                />
              ))}

            {/* Hidden edges - dashed */}
            {showHiddenLines &&
              view.edges
                .filter((e) => e.visibility === "Hidden")
                .map((edge, i) => (
                  <line
                    key={`h-${i}`}
                    x1={edge.start.x}
                    y1={-edge.start.y}
                    x2={edge.end.x}
                    y2={-edge.end.y}
                    stroke={isSelected ? colors.selection : colors.hiddenEdge}
                    strokeWidth={hiddenStrokeWidth}
                    strokeDasharray={`${strokeWidth * 3},${strokeWidth * 2}`}
                    strokeLinecap="round"
                    opacity={isSelected ? 0.7 : 1}
                  />
                ))}
          </g>
        );
      })}

      {/* Dimension annotations */}
      {dimensions.map((dim, di) => (
        <g key={`dim-${di}`} className="dimension" pointerEvents="none">
          {/* Dimension lines */}
          {dim.lines.map(([start, end], li) => (
            <line
              key={`l-${li}`}
              x1={start.x}
              y1={-start.y}
              x2={end.x}
              y2={-end.y}
              stroke={colors.dimension}
              strokeWidth={dimStrokeWidth}
            />
          ))}

          {/* Arcs (for angular dimensions) */}
          {dim.arcs.map((arc, ai) => {
            const startX = arc.center.x + arc.radius * Math.cos(arc.start_angle);
            const startY = -(arc.center.y + arc.radius * Math.sin(arc.start_angle));
            const endX = arc.center.x + arc.radius * Math.cos(arc.end_angle);
            const endY = -(arc.center.y + arc.radius * Math.sin(arc.end_angle));
            const largeArc = Math.abs(arc.end_angle - arc.start_angle) > Math.PI ? 1 : 0;
            const sweep = arc.end_angle > arc.start_angle ? 0 : 1;

            return (
              <path
                key={`arc-${ai}`}
                d={`M ${startX} ${startY} A ${arc.radius} ${arc.radius} 0 ${largeArc} ${sweep} ${endX} ${endY}`}
                fill="none"
                stroke={colors.dimension}
                strokeWidth={dimStrokeWidth}
              />
            );
          })}

          {/* Text labels */}
          {dim.texts.map((t, ti) => (
            <text
              key={`t-${ti}`}
              x={t.position.x}
              y={-t.position.y}
              fontSize={t.height / Math.sqrt(zoom)}
              fill={colors.dimension}
              textAnchor="middle"
              dominantBaseline="middle"
              fontFamily="monospace"
              fontWeight={isDark ? "500" : "400"}
              transform={`rotate(${(-t.rotation * 180) / Math.PI}, ${t.position.x}, ${-t.position.y})`}
            >
              {t.text}
            </text>
          ))}

          {/* Arrows */}
          {dim.arrows.map((arrow, ai) => {
            const angle = arrow.direction;
            const size = arrow.size / Math.sqrt(zoom);
            const tipX = arrow.tip.x;
            const tipY = -arrow.tip.y;
            const halfAngle = Math.PI / 6;
            const p1x = tipX + size * Math.cos(angle + Math.PI - halfAngle);
            const p1y = tipY - size * Math.sin(angle + Math.PI - halfAngle);
            const p2x = tipX + size * Math.cos(angle + Math.PI + halfAngle);
            const p2y = tipY - size * Math.sin(angle + Math.PI + halfAngle);

            return (
              <polygon
                key={`a-${ai}`}
                points={`${tipX},${tipY} ${p1x},${p1y} ${p2x},${p2y}`}
                fill={colors.dimension}
              />
            );
          })}
        </g>
      ))}

      {/* Detail view region outlines on main view */}
      {detailViews.map((def) => {
        const halfW = def.width / 2;
        const halfH = def.height / 2;
        return (
          <g key={def.id} pointerEvents="none">
            {/* Circle around the detail region */}
            <circle
              cx={def.centerX}
              cy={-def.centerY}
              r={Math.max(halfW, halfH) * 1.2}
              fill="none"
              stroke={colors.detailRegion}
              strokeWidth={strokeWidth * 0.8}
              strokeDasharray={`${strokeWidth * 2},${strokeWidth}`}
            />
            {/* Label */}
            <text
              x={def.centerX}
              y={-def.centerY - Math.max(halfW, halfH) * 1.2 - strokeWidth * 3}
              fontSize={strokeWidth * 6}
              fill={colors.detailRegion}
              textAnchor="middle"
              fontFamily="monospace"
              fontWeight="600"
            >
              {def.label}
            </text>
          </g>
        );
      })}

      {/* Section cut lines on the main view */}
      {computedSections.map(({ def }) => {
        const pts = def.points as Array<[number, number]>;
        if (pts.length < 2) return null;
        const first = pts[0]!;
        const last = pts[pts.length - 1]!;
        return (
          <g key={`cut-${def.id}`} pointerEvents="none">
            <polyline
              points={pts.map(([x, y]) => `${x},${-y}`).join(" ")}
              fill="none"
              stroke={colors.section}
              strokeWidth={strokeWidth}
              strokeDasharray={`${strokeWidth * 6},${strokeWidth * 2},${strokeWidth * 2},${strokeWidth * 2}`}
            />
            {[first, last].map(([x, y], i) => (
              <text
                key={`cut-${def.id}-t-${i}`}
                x={x}
                y={-y - strokeWidth * 4}
                fontSize={strokeWidth * 6}
                fill={colors.section}
                textAnchor="middle"
                fontFamily="monospace"
                fontWeight="600"
              >
                {def.label}
              </text>
            ))}
          </g>
        );
      })}

      {/* In-progress section polyline */}
      {isCreatingSection && sectionPoints.length > 0 && (
        <g pointerEvents="none">
          <polyline
            points={[...sectionPoints, ...(sectionCursor ? [[sectionCursor.x, sectionCursor.y]] : [])]
              .map(([x, y]) => `${x},${-(y as number)}`)
              .join(" ")}
            fill="none"
            stroke={colors.section}
            strokeWidth={strokeWidth}
            strokeDasharray={`${strokeWidth * 3},${strokeWidth * 2}`}
          />
          {sectionPoints.map(([x, y], i) => (
            <circle key={`sp-${i}`} cx={x} cy={-y} r={strokeWidth * 2} fill={colors.section} />
          ))}
        </g>
      )}

      {/* Selection rectangle while creating detail view */}
      {isCreatingDetail && detailStart && detailEnd && (
        <rect
          x={Math.min(detailStart.x, detailEnd.x)}
          y={-Math.max(detailStart.y, detailEnd.y)}
          width={Math.abs(detailEnd.x - detailStart.x)}
          height={Math.abs(detailEnd.y - detailStart.y)}
          fill={colors.detailRegionFill}
          stroke={colors.detailRegion}
          strokeWidth={strokeWidth}
          strokeDasharray={`${strokeWidth * 3},${strokeWidth * 2}`}
          pointerEvents="none"
        />
      )}

      {/* View label */}
      <text
        x={viewX + 5 / zoom}
        y={viewY + 5 / zoom + strokeWidth * 8}
        fontSize={strokeWidth * 8}
        fill={colors.label}
        textAnchor="start"
        dominantBaseline="hanging"
        fontFamily="monospace"
        fontWeight="500"
        pointerEvents="none"
      >
        {viewDirection.toUpperCase()} VIEW
      </text>

      {/* Zoom indicator */}
      <text
        x={viewX + viewWidth - 5 / zoom}
        y={viewY + 5 / zoom + strokeWidth * 8}
        fontSize={strokeWidth * 6}
        fill={colors.label}
        textAnchor="end"
        dominantBaseline="hanging"
        fontFamily="monospace"
        pointerEvents="none"
      >
        {Math.round(zoom * 100)}%
      </text>

      {/* Render computed detail views in bottom-right area */}
      {computedDetailViews.map(({ def, view }, idx) => {
        // Position detail views in bottom-right, stacking vertically
        const detailSize = Math.min(viewWidth, viewHeight) * 0.25;
        const padding = detailSize * 0.1;
        const detailX = viewX + viewWidth - detailSize - padding;
        const detailY =
          viewY + viewHeight - detailSize * (idx + 1) - padding * (idx + 1) - tbClearance;

        // Scale to fit in the detail box
        const detailWidth = view.bounds.max_x - view.bounds.min_x;
        const detailHeight = view.bounds.max_y - view.bounds.min_y;
        const fitScale = Math.min(
          (detailSize * 0.8) / (detailWidth || 1),
          (detailSize * 0.8) / (detailHeight || 1)
        );

        const detailCenterX = detailX + detailSize / 2;
        const detailCenterY = detailY + detailSize / 2;

        return (
          <g key={def.id}>
            {/* Detail view border */}
            <rect
              x={detailX}
              y={detailY}
              width={detailSize}
              height={detailSize}
              fill={colors.background}
              stroke={colors.detailRegion}
              strokeWidth={strokeWidth}
            />

            {/* Detail view label */}
            <text
              x={detailX + strokeWidth * 2}
              y={detailY + strokeWidth * 5}
              fontSize={strokeWidth * 5}
              fill={colors.detailRegion}
              fontFamily="monospace"
              fontWeight="600"
              pointerEvents="none"
            >
              DETAIL {def.label} ({def.scale}:1)
            </text>

            {/* Clipped group for detail edges */}
            <g transform={`translate(${detailCenterX}, ${detailCenterY}) scale(${fitScale})`}>
              {view.edges
                .filter((e) => e.visibility === "Visible")
                .map((edge, i) => (
                  <line
                    key={`dv-${def.id}-v-${i}`}
                    x1={edge.start.x}
                    y1={-edge.start.y}
                    x2={edge.end.x}
                    y2={-edge.end.y}
                    stroke={colors.edge}
                    strokeWidth={strokeWidth / fitScale}
                    strokeLinecap="round"
                  />
                ))}
              {showHiddenLines &&
                view.edges
                  .filter((e) => e.visibility === "Hidden")
                  .map((edge, i) => (
                    <line
                      key={`dv-${def.id}-h-${i}`}
                      x1={edge.start.x}
                      y1={-edge.start.y}
                      x2={edge.end.x}
                      y2={-edge.end.y}
                      stroke={colors.hiddenEdge}
                      strokeWidth={hiddenStrokeWidth / fitScale}
                      strokeDasharray={`${(strokeWidth * 3) / fitScale},${(strokeWidth * 2) / fitScale}`}
                      strokeLinecap="round"
                    />
                  ))}
            </g>
          </g>
        );
      })}

      {/* Section view insets, stacked bottom-left */}
      {computedSections.map(({ def, view }, idx) => {
        const boxSize = Math.min(viewWidth, viewHeight) * 0.28;
        const pad = boxSize * 0.1;
        const boxX = viewX + pad;
        const boxY = viewY + viewHeight - boxSize * (idx + 1) - pad * (idx + 1);

        const secW = view.bounds.max_x - view.bounds.min_x;
        const secH = view.bounds.max_y - view.bounds.min_y;
        const fitScale = Math.min(
          (boxSize * 0.8) / (secW || 1),
          (boxSize * 0.8) / (secH || 1),
        );
        const cx = (view.bounds.min_x + view.bounds.max_x) / 2;
        const cy = (view.bounds.min_y + view.bounds.max_y) / 2;
        const boxCx = boxX + boxSize / 2;
        const boxCy = boxY + boxSize / 2;

        return (
          <g key={`sec-${def.id}`} pointerEvents="none">
            <rect
              x={boxX}
              y={boxY}
              width={boxSize}
              height={boxSize}
              fill={colors.background}
              stroke={colors.section}
              strokeWidth={strokeWidth}
            />
            <text
              x={boxX + strokeWidth * 2}
              y={boxY + strokeWidth * 5}
              fontSize={strokeWidth * 5}
              fill={colors.section}
              fontFamily="monospace"
              fontWeight="600"
            >
              SECTION {def.label}-{def.label}
            </text>
            <g
              transform={`translate(${boxCx - cx * fitScale}, ${boxCy + cy * fitScale}) scale(${fitScale})`}
            >
              {view.curves.map((curve, ci) => (
                <polyline
                  key={`sec-${def.id}-c-${ci}`}
                  points={[
                    ...curve.points,
                    ...(curve.is_closed && curve.points.length > 0 ? [curve.points[0]!] : []),
                  ]
                    .map((p) => `${p.x},${-p.y}`)
                    .join(" ")}
                  fill="none"
                  stroke={colors.edge}
                  strokeWidth={(strokeWidth * 1.2) / fitScale}
                  strokeLinecap="round"
                />
              ))}
              {view.hatch_lines.map(([a, b], hi) => (
                <line
                  key={`sec-${def.id}-h-${hi}`}
                  x1={a.x}
                  y1={-a.y}
                  x2={b.x}
                  y2={-b.y}
                  stroke={colors.section}
                  strokeWidth={(strokeWidth * 0.5) / fitScale}
                />
              ))}
            </g>
          </g>
        );
      })}

      {/* Title block, bottom-right (kernel-rendered, matches the PDF) */}
      {titleBlock &&
        renderBlock(
          titleBlock,
          viewX + viewWidth - titleBlock.width * tbScale - sheetPad,
          viewY + viewHeight - sheetPad,
          tbScale,
          colors.sheet,
          strokeWidth * 0.5,
          "titleblock",
        )}

      {/* BOM table, above the title block */}
      {bomBlock &&
        renderBlock(
          bomBlock,
          viewX + viewWidth - bomBlock.width * bomScale - sheetPad,
          viewY + viewHeight - tbClearance - sheetPad * 2,
          bomScale,
          colors.sheet,
          strokeWidth * 0.5,
          "bom",
        )}
    </svg>
  );
}
