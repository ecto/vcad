import { useState, useRef, useEffect, useMemo } from "react";
import * as RadixContextMenu from "@radix-ui/react-context-menu";
import {
  DndContext,
  DragOverlay,
  closestCenter,
  KeyboardSensor,
  MouseSensor,
  TouchSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Cylinder } from "@phosphor-icons/react/dist/ssr/Cylinder";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { Sliders } from "@phosphor-icons/react/dist/ssr/Sliders";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Intersect } from "@phosphor-icons/react/dist/ssr/Intersect";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { CaretDown } from "@phosphor-icons/react/dist/ssr/CaretDown";
import { ArrowUp } from "@phosphor-icons/react/dist/ssr/ArrowUp";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { Spiral } from "@phosphor-icons/react/dist/ssr/Spiral";
import { Stack } from "@phosphor-icons/react/dist/ssr/Stack";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { LinkSimple } from "@phosphor-icons/react/dist/ssr/LinkSimple";
import { Anchor } from "@phosphor-icons/react/dist/ssr/Anchor";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Octagon } from "@phosphor-icons/react/dist/ssr/Octagon";
import { CubeTransparent } from "@phosphor-icons/react/dist/ssr/CubeTransparent";
import { DotsThree } from "@phosphor-icons/react/dist/ssr/DotsThree";
import { ArrowsHorizontal } from "@phosphor-icons/react/dist/ssr/ArrowsHorizontal";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { EyeSlash } from "@phosphor-icons/react/dist/ssr/EyeSlash";
import { TextT } from "@phosphor-icons/react/dist/ssr/TextT";
import { Circuitry } from "@phosphor-icons/react/dist/ssr/Circuitry";
import { Scissors } from "@phosphor-icons/react/dist/ssr/Scissors";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { ContextMenu } from "@/components/ContextMenu";
import { useDocumentStore, useUiStore, useSketchStore, useParametersStore, isBooleanPart, isPrimitivePart, isSweepPart, isExtrudePart, isRevolvePart, isFilletPart, isChamferPart, isShellPart, isEmbroideryPatternPart, isStitchPart, isPcbBoardPart, t, tFmt } from "@vcad/core";
import { useLocaleStore } from "@/stores/locale-store";
import { useNotificationStore } from "@/stores/notification-store";
import { SketchPropertyPanel } from "@/components/SketchPropertyPanel";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";
import type { PrimitiveKind, PartInfo, BooleanPartInfo, PrimitivePartInfo, SweepPartInfo, ExtrudePartInfo, RevolvePartInfo, FilletPartInfo, ChamferPartInfo, ShellPartInfo } from "@vcad/core";
import type { PartInstance, Joint, JointKind } from "@vcad/ir";
import { cn } from "@/lib/utils";
import { getPartSummary } from "./tree/part-summary";
import { InlineCubeDimensions, InlineCylinderDimensions, InlineSphereDimensions, InlineExtrudeDimensions, InlineRevolveDimensions, InlineFilletDimensions, InlineChamferDimensions, InlineShellDimensions, InlineSweepProperties } from "./tree/InlineDimensions";
import { InlinePositionSection, InlineRotationSection } from "./tree/InlineTransform";
import { InlineMaterial } from "./tree/InlineMaterial";
import { EmbroideryProperties } from "./embroidery/EmbroideryProperties";
function SceneTreeRow() {
  const inspectorTarget = useUiStore((s) => s.inspectorTarget);
  const setInspectorTarget = useUiStore((s) => s.setInspectorTarget);
  const setSidebarPane = useUiStore((s) => s.setSidebarPane);
  const clearSelection = useUiStore((s) => s.clearSelection);
  useLocaleStore((s) => s.locale);
  const active = inspectorTarget?.kind === "scene";
  return (
    <button
      onClick={() => {
        clearSelection();
        setInspectorTarget({ kind: "scene" });
        setSidebarPane("inspector");
      }}
      className={cn(
        "flex w-full items-center gap-2 px-2 h-7 text-xs",
        "hover:bg-hover",
        active ? "text-brand bg-brand/10" : "text-text",
      )}
      title={t("tree.scene_tooltip")}
    >
      <Globe size={13} className={active ? "text-brand" : "text-text-muted"} />
      <span className="font-medium">{t("tree.scene")}</span>
    </button>
  );
}

function ParametersTreeRow() {
  const setSidebarPane = useUiStore((s) => s.setSidebarPane);
  const setInspectorTarget = useUiStore((s) => s.setInspectorTarget);
  const parameterCount = useParametersStore((s) => Object.keys(s.parameters).length);
  useLocaleStore((s) => s.locale);
  return (
    <button
      onClick={() => {
        setInspectorTarget(null);
        setSidebarPane("parameters");
      }}
      className={cn(
        "flex w-full items-center gap-2 px-2 h-7 text-xs",
        "hover:bg-hover text-text",
      )}
      title={t("tree.parameters_tooltip")}
    >
      <Sliders size={13} className="text-text-muted" />
      <span className="font-medium">{t("tree.parameters")}</span>
      {parameterCount > 0 && (
        <span className="text-[10px] text-text-muted tabular-nums">
          {parameterCount}
        </span>
      )}
    </button>
  );
}

/**
 * In-tree summary of the active sketch. The single home for everything sketch:
 *   - Finish ✓ / Cancel × header (replaces the old floating corner overlay)
 *   - SketchPropertyPanel (operation params + tool/entity/constraint editor)
 *   - Entities / Constraints / Profiles trees
 *
 * Renders nothing when sketch is inactive — the tree falls back to its
 * normal Scene + Parts content. Living in the FeatureTree means the right
 * sidebar stays free for ChatSidebar.
 */
function SketchTreeSection() {
  const active = useSketchStore((s) => s.active);
  const segments = useSketchStore((s) => s.segments);
  const constraints = useSketchStore((s) => s.constraints);
  const constraintStatus = useSketchStore((s) => s.constraintStatus);
  const profiles = useSketchStore((s) => s.profiles);
  const loftMode = useSketchStore((s) => s.loftMode);
  const pendingExit = useSketchStore((s) => s.pendingExit);
  const pendingOperation = useSketchStore((s) => s.pendingOperation);
  const selectedSegments = useSketchStore((s) => s.selectedSegments);
  const selectedConstraintIndex = useSketchStore((s) => s.selectedConstraintIndex);
  const toggleSegmentSelection = useSketchStore((s) => s.toggleSegmentSelection);
  const setSelectedConstraint = useSketchStore((s) => s.setSelectedConstraint);
  const removeConstraint = useSketchStore((s) => s.removeConstraint);
  const requestExit = useSketchStore((s) => s.requestExit);
  const confirmExit = useSketchStore((s) => s.confirmExit);
  const cancelExit = useSketchStore((s) => s.cancelExit);
  const addToast = useNotificationStore((s) => s.addToast);
  const [entitiesOpen, setEntitiesOpen] = useState(true);
  const [constraintsOpen, setConstraintsOpen] = useState(true);
  const [profilesOpen, setProfilesOpen] = useState(true);
  useLocaleStore((s) => s.locale);

  if (!active) return null;

  const hasSegments = segments.length > 0;
  const finishLabel = pendingOperation
    ? pendingOperation.kind.charAt(0).toUpperCase() + pendingOperation.kind.slice(1)
    : t("tree.sketch.finish");
  const finishEnabled = hasSegments || pendingOperation?.kind === "loft";

  return (
    <div className="border border-amber-500/30 bg-amber-500/5 mt-1 mb-2">
      <div className="flex items-center justify-between gap-2 px-2 py-1.5 border-b border-amber-500/20 bg-amber-500/5">
        <span className="text-[10px] font-medium uppercase tracking-wider text-amber-400">
          {t("tree.sketch.label")}
        </span>
        <span
          className={cn(
            "text-[9px] uppercase tracking-wide tabular-nums",
            constraintStatus === "solved" && "text-emerald-400",
            constraintStatus === "error" && "text-red-400",
            constraintStatus === "over" && "text-orange-400",
            constraintStatus === "under" && "text-yellow-400/70",
          )}
          title={`${constraints.length} constraint${constraints.length === 1 ? "" : "s"} · ${constraintStatus}`}
        >
          {constraints.length}c · {constraintStatus}
        </span>
      </div>
      <div className="flex gap-1 px-2 py-1.5 border-b border-amber-500/20">
        <button
          type="button"
          onClick={() => window.dispatchEvent(new CustomEvent("vcad:sketch-commit"))}
          disabled={!finishEnabled}
          className={cn(
            "flex-1 flex items-center justify-center gap-1 px-2 py-1 text-xs font-medium",
            finishEnabled
              ? "bg-emerald-600 hover:bg-emerald-700 text-white"
              : "bg-hover/40 text-text-muted cursor-not-allowed",
          )}
          title={pendingOperation ? tFmt("tree.sketch.apply_tooltip", { label: finishLabel }) : t("tree.sketch.finish_tooltip")}
        >
          <span>✓</span>
          <span>{finishLabel}</span>
        </button>
        <button
          type="button"
          onClick={() => {
            const exited = requestExit();
            if (exited) addToast(t("tree.sketch.cancelled"), "info");
          }}
          className="px-2 py-1 text-xs text-text-muted hover:text-text hover:bg-hover/60"
          title={t("tree.sketch.cancel_tooltip")}
        >
          ×
        </button>
      </div>

      {pendingExit && (
        <div className="px-2 py-2 border-b border-amber-500/20 bg-red-500/10 text-xs">
          <div className="flex items-center gap-1 text-amber-300 mb-1">
            <span>⚠</span>
            <span className="font-medium">{t("tree.sketch.discard_title")}</span>
          </div>
          <div className="text-[11px] text-text-muted mb-2">
            {t("tree.sketch.discard_msg")}
          </div>
          <div className="flex gap-1">
            <button
              type="button"
              onClick={() => {
                confirmExit();
                addToast(t("tree.sketch.discarded"), "info");
              }}
              className="flex-1 px-2 py-1 text-xs bg-red-600 hover:bg-red-700 text-white"
            >
              {t("tree.sketch.discard")}
            </button>
            <button
              type="button"
              onClick={cancelExit}
              className="flex-1 px-2 py-1 text-xs hover:bg-hover/60"
            >
              {t("tree.sketch.keep_editing")}
            </button>
          </div>
        </div>
      )}

      {/* Operation params + tool/entity/constraint editor — same component as
          before, just rendered inline inside the card. */}
      <SketchPropertyPanel />

      <div className="space-y-0.5 pb-1 border-t border-amber-500/20">
        <button
          onClick={() => setEntitiesOpen((o) => !o)}
          className="flex w-full items-center gap-1 px-2 h-6 text-[11px] text-text-muted hover:text-text"
        >
          <span className="font-mono">{entitiesOpen ? "▾" : "▸"}</span>
          <span>{tFmt("tree.sketch.entities", { count: String(segments.length) })}</span>
        </button>
        {entitiesOpen &&
          segments.map((seg, i) => (
            <button
              key={i}
              onClick={() => toggleSegmentSelection(i)}
              className={cn(
                "flex w-full items-center gap-2 pl-6 pr-2 h-6 text-[11px]",
                selectedSegments.includes(i)
                  ? "bg-amber-500/20 text-amber-300"
                  : "text-text-muted hover:bg-hover hover:text-text",
              )}
            >
              <span className="font-mono">{seg.type === "Line" ? "—" : "◜"}</span>
              <span>
                {seg.type} {i + 1}
              </span>
            </button>
          ))}

        <button
          onClick={() => setConstraintsOpen((o) => !o)}
          className="flex w-full items-center gap-1 px-2 h-6 text-[11px] text-text-muted hover:text-text"
        >
          <span className="font-mono">{constraintsOpen ? "▾" : "▸"}</span>
          <span>{tFmt("tree.sketch.constraints", { count: String(constraints.length) })}</span>
        </button>
        {constraintsOpen &&
          constraints.map((c, i) => (
            <div
              key={i}
              className={cn(
                "flex items-center gap-2 pl-6 pr-2 h-6 text-[11px]",
                selectedConstraintIndex === i
                  ? "bg-amber-500/20 text-amber-300"
                  : "text-text-muted hover:bg-hover hover:text-text",
              )}
            >
              <button
                onClick={() => setSelectedConstraint(i)}
                className="flex-1 text-left flex items-center gap-2"
              >
                <span className="font-mono">⌐</span>
                <span>{c.type}</span>
              </button>
              <button
                onClick={() => {
                  removeConstraint(i);
                  if (selectedConstraintIndex === i) setSelectedConstraint(null);
                }}
                className="text-text-muted hover:text-red-400"
                title={t("tree.sketch.delete_constraint")}
                aria-label={t("tree.sketch.delete_constraint")}
              >
                ×
              </button>
            </div>
          ))}

        {loftMode && (
          <>
            <button
              onClick={() => setProfilesOpen((o) => !o)}
              className="flex w-full items-center gap-1 px-2 h-6 text-[11px] text-text-muted hover:text-text"
            >
              <span className="font-mono">{profilesOpen ? "▾" : "▸"}</span>
              <span>{tFmt("tree.sketch.profiles", { count: String(profiles.length) })}</span>
            </button>
            {profilesOpen &&
              profiles.map((p, i) => (
                <div
                  key={p.id}
                  className="flex items-center gap-2 pl-6 pr-2 h-6 text-[11px] text-text-muted"
                >
                  <span className="font-mono">≡</span>
                  <span>{tFmt("tree.sketch.profile", { n: String(i + 1) })}</span>
                </div>
              ))}
          </>
        )}
      </div>
    </div>
  );
}

const KIND_ICONS: Record<PrimitiveKind, typeof Cube> = {
  cube: Cube,
  cylinder: Cylinder,
  sphere: Globe,
};

/**
 * Empty-state shown in the feature tree when the document has no parts or
 * instances yet. Three quick-add tiles for the most common primitives plus a
 * one-line nudge toward ⌘K and the AI chat.
 */
function FeatureTreeEmptyState() {
  const addPrimitive = useDocumentStore((s) => s.addPrimitive);
  const setCommandPaletteOpen = useUiStore((s) => s.setCommandPaletteOpen);

  const tiles: { kind: PrimitiveKind; icon: typeof Cube; label: string }[] = [
    { kind: "cube", icon: Cube, label: "Box" },
    { kind: "cylinder", icon: Cylinder, label: "Cylinder" },
    { kind: "sphere", icon: Globe, label: "Sphere" },
  ];

  return (
    <div className="px-2 py-3 flex flex-col items-center gap-3 text-center">
      <div className="text-[11px] text-text-muted leading-tight">
        Your document is empty.<br />Drop in a primitive to get started.
      </div>
      <div className="grid grid-cols-3 gap-1.5 w-full">
        {tiles.map(({ kind, icon: Icon, label }) => (
          <button
            key={kind}
            onClick={() => addPrimitive(kind)}
            className="flex flex-col items-center justify-center gap-1 aspect-square border border-border bg-card hover:bg-hover hover:border-brand/50 transition-colors"
            title={`Add ${label}`}
          >
            <Icon size={20} className="text-text-muted" />
            <span className="text-[10px] text-text">{label}</span>
          </button>
        ))}
      </div>
      <button
        onClick={() => setCommandPaletteOpen(true)}
        className="text-[10px] text-text-muted hover:text-brand transition-colors"
      >
        or press <span className="font-mono px-1 bg-hover text-text">⌘K</span> to search or ask AI
      </button>
    </div>
  );
}

function getPartIcon(part: PartInfo): typeof Cube {
  if (part.kind === "boolean") return Intersect;
  if (part.kind === "extrude") return ArrowUp;
  if (part.kind === "revolve") return ArrowsClockwise;
  if (part.kind === "sweep") return Spiral;
  if (part.kind === "loft") return Stack;
  if (part.kind === "imported-mesh") return Package;
  if (part.kind === "fillet") return Circle;
  if (part.kind === "chamfer") return Octagon;
  if (part.kind === "shell") return CubeTransparent;
  if (part.kind === "linear-pattern") return DotsThree;
  if (part.kind === "circular-pattern") return ArrowsClockwise;
  if (part.kind === "mirror") return ArrowsHorizontal;
  if (part.kind === "text") return TextT;
  if (part.kind === "pcb-board") return Circuitry;
  if (part.kind === "embroidery-pattern") return Scissors;
  if (part.kind === "stitch") return Scissors;
  return KIND_ICONS[part.kind];
}

/** Drag preview shown in DragOverlay - renders outside sidebar constraints */
function DragPreview({ part }: { part: PartInfo }) {
  const Icon = getPartIcon(part);
  return (
    <div className="flex items-center gap-1.5 px-2 py-1 bg-surface border border-brand rounded shadow-lg text-xs text-text">
      <Icon size={12} className="shrink-0 text-text-muted" />
      <span className="truncate max-w-32">{part.name}</span>
    </div>
  );
}

function InlineRenameInput({
  partId,
  currentName,
  onDone,
}: {
  partId: string;
  currentName: string;
  onDone: () => void;
}) {
  const [text, setText] = useState(currentName);
  const inputRef = useRef<HTMLInputElement>(null);
  const renamePart = useDocumentStore((s) => s.renamePart);

  useEffect(() => {
    inputRef.current?.select();
  }, []);

  function commit() {
    const trimmed = text.trim();
    if (trimmed && trimmed !== currentName) {
      renamePart(partId, trimmed);
    }
    onDone();
  }

  return (
    <input
      ref={inputRef}
      type="text"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") onDone();
      }}
      className="flex-1 border border-brand bg-surface px-1 py-0.5 text-xs text-text outline-none w-0"
      autoFocus
    />
  );
}

interface TreeNodeProps {
  part: PartInfo;
  depth: number;
  expandedIds: Set<string>;
  toggleExpanded: (id: string) => void;
  consumedParts: Record<string, PartInfo>;
  renamingId: string | null;
  setRenamingId: (id: string | null) => void;
  /** IDs of parts that are expanded for inline editing */
  inlineExpandedIds: Set<string>;
  toggleInlineExpanded: (id: string) => void;
  /** Drag listeners (only for depth 0) */
  dragListeners?: React.HTMLAttributes<HTMLElement>;
  /** Whether this node is being dragged */
  isDragging?: boolean;
}

function TreeNode({
  part,
  depth,
  expandedIds,
  toggleExpanded,
  consumedParts,
  renamingId,
  setRenamingId,
  inlineExpandedIds,
  toggleInlineExpanded,
  dragListeners,
  isDragging,
}: TreeNodeProps) {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const hoveredPartId = useUiStore((s) => s.hoveredPartId);
  const setHoveredPartId = useUiStore((s) => s.setHoveredPartId);
  const select = useUiStore((s) => s.select);
  const toggleSelect = useUiStore((s) => s.toggleSelect);
  const clearSelection = useUiStore((s) => s.clearSelection);
  const removePart = useDocumentStore((s) => s.removePart);
  const document = useDocumentStore((s) => s.document);
  const setPartVisible = useDocumentStore((s) => s.setPartVisible);

  const Icon = getPartIcon(part);
  const isSelected = selectedPartIds.has(part.id);
  const isHovered = hoveredPartId === part.id;
  const isRenaming = renamingId === part.id;

  const isBoolean = isBooleanPart(part);
  const hasChildren = isBoolean && part.sourcePartIds.length > 0;
  const isExpanded = expandedIds.has(part.id);
  const isInlineExpanded = inlineExpandedIds.has(part.id);

  // Check if this part is visible
  const rootEntry = document.roots.find((r) => r.root === part.translateNodeId);
  const isVisible = rootEntry?.visible !== false;

  // Allow inline expansion for all top-level parts (not just primitives)
  const canInlineExpand = depth === 0;

  // Get summary text for collapsed state
  const summary = getPartSummary(part, document);

  // Get transform data for inline editing
  const translateNode = document.nodes[String(part.translateNodeId)];
  const rotateNode = document.nodes[String(part.rotateNodeId)];
  const offset =
    translateNode?.op.type === "Translate"
      ? translateNode.op.offset
      : { x: 0, y: 0, z: 0 };
  const angles =
    rotateNode?.op.type === "Rotate"
      ? rotateNode.op.angles
      : { x: 0, y: 0, z: 0 };

  // Get material for inline picker
  const materialKey = rootEntry?.material ?? "default";

  const childParts = useMemo(() => {
    if (!isBoolean) return [];
    return (part as BooleanPartInfo).sourcePartIds
      .map((id) => consumedParts[id])
      .filter((p): p is PartInfo => p !== undefined);
  }, [isBoolean, part, consumedParts]);

  // Render inline dimensions if expanded
  function renderInlineDimensions() {
    if (isPrimitivePart(part)) {
      const primPart = part as PrimitivePartInfo;
      switch (primPart.kind) {
        case "cube":
          return <InlineCubeDimensions part={primPart} />;
        case "cylinder":
          return <InlineCylinderDimensions part={primPart} />;
        case "sphere":
          return <InlineSphereDimensions part={primPart} />;
      }
    }
    if (isExtrudePart(part)) {
      return <InlineExtrudeDimensions part={part as ExtrudePartInfo} />;
    }
    if (isRevolvePart(part)) {
      return <InlineRevolveDimensions part={part as RevolvePartInfo} />;
    }
    if (isFilletPart(part)) {
      return <InlineFilletDimensions part={part as FilletPartInfo} />;
    }
    if (isChamferPart(part)) {
      return <InlineChamferDimensions part={part as ChamferPartInfo} />;
    }
    if (isShellPart(part)) {
      return <InlineShellDimensions part={part as ShellPartInfo} />;
    }
    if (isSweepPart(part)) {
      return <InlineSweepProperties part={part as SweepPartInfo} />;
    }
    return null;
  }

  return (
    <>
      <div
        className={cn(
          "group flex items-center gap-1 px-2 py-1 text-xs cursor-pointer rounded",
          isSelected
            ? "bg-brand/20 text-brand backdrop-blur-sm"
            : isHovered
            ? "bg-surface/80 text-text backdrop-blur-sm"
            : "text-text-muted/90 hover:bg-surface/60 hover:text-text hover:backdrop-blur-sm",
          depth > 0 && "opacity-70",
          !isVisible && "opacity-40",
          isDragging && "opacity-50",
        )}
        style={{ paddingLeft: `${8 + depth * 16}px` }}
        onClick={(e) => {
          if (isRenaming) return;
          if (depth > 0) return;
          if (e.shiftKey) {
            toggleSelect(part.id);
          } else {
            select(part.id);
          }
        }}
        onDoubleClick={() => {
          if (depth !== 0) return;
          if (isPcbBoardPart(part)) {
            useElectronicsStore.getState().enter();
          } else if (isStitchPart(part) || isEmbroideryPatternPart(part)) {
            select(part.id);
            useEmbroideryStore.getState().openPanel();
          } else {
            setRenamingId(part.id);
          }
        }}
        onMouseEnter={() => setHoveredPartId(part.id)}
        onMouseLeave={() => setHoveredPartId(null)}
        {...(depth === 0 ? dragListeners : {})}
      >
        {/* Expand caret for boolean children OR inline dimensions */}
        {(hasChildren || canInlineExpand) ? (
          <button
            onClick={(e) => {
              e.stopPropagation();
              if (hasChildren) {
                toggleExpanded(part.id);
              } else if (canInlineExpand) {
                toggleInlineExpanded(part.id);
              }
            }}
            className="shrink-0 p-0.5 hover:bg-hover"
          >
            {(hasChildren ? isExpanded : isInlineExpanded) ? (
              <CaretDown size={10} />
            ) : (
              <CaretRight size={10} />
            )}
          </button>
        ) : (
          <span className="w-4" />
        )}
        <Icon size={14} className="shrink-0" />
        {isRenaming ? (
          <InlineRenameInput
            partId={part.id}
            currentName={part.name}
            onDone={() => setRenamingId(null)}
          />
        ) : (
          <span className="flex-1 overflow-hidden whitespace-nowrap">
            {part.name}
            {/* Show summary when not inline expanded */}
            {!isInlineExpanded && summary && (
              <span className="ml-1 text-text-muted/60 text-[10px]">{summary}</span>
            )}
          </span>
        )}
        {depth === 0 && (
          <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100">
            {/* Quick-edit workspace button */}
            {isPcbBoardPart(part) && (
              <Tooltip content="Edit Circuit">
                <Button variant="ghost" size="icon-sm" className="h-5 w-5"
                  onClick={(e) => { e.stopPropagation(); useElectronicsStore.getState().enter(); }}
                >
                  <PencilSimple size={12} />
                </Button>
              </Tooltip>
            )}
            {(isStitchPart(part) || isEmbroideryPatternPart(part)) && (
              <Tooltip content="Edit Embroidery">
                <Button variant="ghost" size="icon-sm" className="h-5 w-5"
                  onClick={(e) => {
                    e.stopPropagation();
                    select(part.id);
                    useEmbroideryStore.getState().openPanel();
                  }}
                >
                  <PencilSimple size={12} />
                </Button>
              </Tooltip>
            )}
            {/* Visibility toggle */}
            <Tooltip content={isVisible ? "Hide" : "Show"}>
              <Button
                variant="ghost"
                size="icon-sm"
                className="h-5 w-5"
                onClick={(e) => {
                  e.stopPropagation();
                  setPartVisible(part.id, !isVisible);
                }}
              >
                {isVisible ? <Eye size={12} /> : <EyeSlash size={12} />}
              </Button>
            </Tooltip>
            {/* Delete button */}
            <Tooltip content="Delete">
              <Button
                variant="ghost"
                size="icon-sm"
                className="h-5 w-5"
                onClick={(e) => {
                  e.stopPropagation();
                  removePart(part.id);
                  if (isSelected) clearSelection();
                }}
              >
                <Trash size={12} />
              </Button>
            </Tooltip>
          </div>
        )}
      </div>

      {/* Inline editing panel (when expanded) */}
      {canInlineExpand && isInlineExpanded && (
        <div className="pl-6 space-y-0.5">
          {/* Dimensions (for primitives only) */}
          {renderInlineDimensions()}
          {/* Embroidery properties */}
          {isEmbroideryPatternPart(part) && <EmbroideryProperties part={part} />}
          {/* Position & Rotation */}
          <InlinePositionSection part={part} offset={offset} />
          <InlineRotationSection part={part} angles={angles} />
          {/* Material */}
          <InlineMaterial partId={part.id} currentMaterialKey={materialKey} />
        </div>
      )}

      {/* Boolean children */}
      {hasChildren && isExpanded && (
        <>
          {childParts.map((child) => (
            <TreeNode
              key={child.id}
              part={child}
              depth={depth + 1}
              expandedIds={expandedIds}
              toggleExpanded={toggleExpanded}
              consumedParts={consumedParts}
              renamingId={renamingId}
              setRenamingId={setRenamingId}
              inlineExpandedIds={inlineExpandedIds}
              toggleInlineExpanded={toggleInlineExpanded}
            />
          ))}
        </>
      )}
    </>
  );
}

/** Sortable wrapper for TreeNode at depth 0 */
interface SortableTreeNodeProps extends Omit<TreeNodeProps, "dragListeners" | "isDragging"> {
  id: string;
}

function SortableTreeNode({ id, ...props }: SortableTreeNodeProps) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  return (
    <div ref={setNodeRef} style={style} {...attributes}>
      <TreeNode
        {...props}
        dragListeners={listeners}
        isDragging={isDragging}
      />
    </div>
  );
}

/** Get a display string for joint type */
function getJointTypeLabel(kind: JointKind): string {
  switch (kind.type) {
    case "Fixed":
      return "Fixed";
    case "Revolute":
      return "Revolute";
    case "Slider":
      return "Slider";
    case "Cylindrical":
      return "Cylindrical";
    case "Ball":
      return "Ball";
  }
}

/** Get icon for joint type */
function getJointIcon(kind: JointKind): typeof LinkSimple {
  switch (kind.type) {
    case "Fixed":
      return Anchor;
    case "Revolute":
      return ArrowsClockwise;
    case "Slider":
      return ArrowUp;
    case "Cylindrical":
      return Spiral;
    case "Ball":
      return Globe;
  }
}

/** Context menu item for assembly tree */
function AssemblyMenuItem({
  icon: Icon,
  label,
  disabled,
  onClick,
}: {
  icon: typeof Copy;
  label: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <RadixContextMenu.Item
      className="group flex items-center gap-2 px-2 py-1.5 text-xs text-text outline-none cursor-pointer data-[disabled]:opacity-40 data-[disabled]:cursor-default data-[highlighted]:bg-brand/20 data-[highlighted]:text-brand"
      disabled={disabled}
      onClick={onClick}
    >
      <Icon size={14} className="shrink-0" />
      <span className="flex-1">{label}</span>
    </RadixContextMenu.Item>
  );
}

interface InstanceNodeProps {
  instance: PartInstance;
  joint?: Joint;
  isGround: boolean;
  onRename: (instanceId: string) => void;
}

function InstanceNode({ instance, joint, isGround, onRename }: InstanceNodeProps) {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const hoveredPartId = useUiStore((s) => s.hoveredPartId);
  const setHoveredPartId = useUiStore((s) => s.setHoveredPartId);
  const select = useUiStore((s) => s.select);
  const toggleSelect = useUiStore((s) => s.toggleSelect);
  const deleteInstance = useDocumentStore((s) => s.deleteInstance);
  const setGroundInstance = useDocumentStore((s) => s.setGroundInstance);
  const clearSelection = useUiStore((s) => s.clearSelection);

  const isSelected = selectedPartIds.has(instance.id);
  const isHovered = hoveredPartId === instance.id;

  const displayName = instance.name ?? instance.partDefId;
  const jointSuffix = joint
    ? ` [${getJointTypeLabel(joint.kind)}]`
    : isGround
    ? " (grounded)"
    : "";

  function handleDelete() {
    deleteInstance(instance.id);
    if (isSelected) clearSelection();
  }

  function handleSetGround() {
    setGroundInstance(instance.id);
  }

  return (
    <RadixContextMenu.Root>
      <RadixContextMenu.Trigger asChild>
        <div
          className={cn(
            "group flex items-center gap-1 px-2 py-1 text-xs cursor-pointer rounded",
            isSelected
              ? "bg-brand/20 text-brand backdrop-blur-sm"
              : isHovered
              ? "bg-surface/80 text-text backdrop-blur-sm"
              : "text-text-muted/90 hover:bg-surface/60 hover:text-text hover:backdrop-blur-sm",
          )}
          style={{ paddingLeft: "24px" }}
          onClick={(e) => {
            if (e.shiftKey) {
              toggleSelect(instance.id);
            } else {
              select(instance.id);
            }
          }}
          onMouseEnter={() => setHoveredPartId(instance.id)}
          onMouseLeave={() => setHoveredPartId(null)}
        >
          <Package size={14} className="shrink-0" />
          <span className="flex-1 overflow-hidden whitespace-nowrap">
            {displayName}
            <span className="text-text-muted/70">{jointSuffix}</span>
          </span>
          {isGround && (
            <Anchor size={12} className="shrink-0 text-text-muted/50" />
          )}
        </div>
      </RadixContextMenu.Trigger>
      <RadixContextMenu.Portal>
        <RadixContextMenu.Content className="z-50 min-w-[160px] border border-border bg-card p-1 shadow-xl">
          <AssemblyMenuItem
            icon={PencilSimple}
            label="Rename"
            onClick={() => onRename(instance.id)}
          />
          <AssemblyMenuItem
            icon={Anchor}
            label="Set as Ground"
            disabled={isGround}
            onClick={handleSetGround}
          />
          <RadixContextMenu.Separator className="my-1 h-px bg-border" />
          <AssemblyMenuItem
            icon={Trash}
            label="Delete Instance"
            onClick={handleDelete}
          />
        </RadixContextMenu.Content>
      </RadixContextMenu.Portal>
    </RadixContextMenu.Root>
  );
}

interface JointNodeProps {
  joint: Joint;
  instancesById: Map<string, PartInstance>;
}

function JointNode({ joint, instancesById }: JointNodeProps) {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const hoveredPartId = useUiStore((s) => s.hoveredPartId);
  const setHoveredPartId = useUiStore((s) => s.setHoveredPartId);
  const select = useUiStore((s) => s.select);
  const toggleSelect = useUiStore((s) => s.toggleSelect);
  const deleteJoint = useDocumentStore((s) => s.deleteJoint);
  const clearSelection = useUiStore((s) => s.clearSelection);

  // Use joint.id prefixed with "joint:" to distinguish from instances
  const jointSelectionId = `joint:${joint.id}`;
  const isSelected = selectedPartIds.has(jointSelectionId);
  const isHovered = hoveredPartId === jointSelectionId;

  const Icon = getJointIcon(joint.kind);
  const parentName = joint.parentInstanceId
    ? instancesById.get(joint.parentInstanceId)?.name ?? joint.parentInstanceId
    : "Ground";
  const childName =
    instancesById.get(joint.childInstanceId)?.name ?? joint.childInstanceId;
  const displayName = joint.name ?? `${getJointTypeLabel(joint.kind)} Joint`;

  // Show state value for non-fixed joints
  let stateDisplay = "";
  if (joint.kind.type === "Revolute") {
    stateDisplay = ` ${joint.state.toFixed(0)}°`;
  } else if (joint.kind.type === "Slider") {
    stateDisplay = ` ${joint.state.toFixed(1)}mm`;
  }

  function handleDelete() {
    deleteJoint(joint.id);
    if (isSelected) clearSelection();
  }

  return (
    <RadixContextMenu.Root>
      <RadixContextMenu.Trigger asChild>
        <div
          className={cn(
            "group flex items-center gap-1 px-2 py-1 text-xs cursor-pointer rounded",
            isSelected
              ? "bg-brand/20 text-brand backdrop-blur-sm"
              : isHovered
              ? "bg-surface/80 text-text backdrop-blur-sm"
              : "text-text-muted/90 hover:bg-surface/60 hover:text-text hover:backdrop-blur-sm",
          )}
          style={{ paddingLeft: "24px" }}
          onClick={(e) => {
            if (e.shiftKey) {
              toggleSelect(jointSelectionId);
            } else {
              select(jointSelectionId);
            }
          }}
          onMouseEnter={() => setHoveredPartId(jointSelectionId)}
          onMouseLeave={() => setHoveredPartId(null)}
        >
          <Icon size={14} className="shrink-0" />
          <span className="flex-1 overflow-hidden whitespace-nowrap">
            {displayName}
            <span className="text-text-muted/70">{stateDisplay}</span>
          </span>
          <Tooltip content={`${parentName} → ${childName}`} side="right">
            <LinkSimple size={12} className="shrink-0 text-text-muted/50" />
          </Tooltip>
        </div>
      </RadixContextMenu.Trigger>
      <RadixContextMenu.Portal>
        <RadixContextMenu.Content className="z-50 min-w-[160px] border border-border bg-card p-1 shadow-xl">
          <AssemblyMenuItem
            icon={Trash}
            label="Delete Joint"
            onClick={handleDelete}
          />
        </RadixContextMenu.Content>
      </RadixContextMenu.Portal>
    </RadixContextMenu.Root>
  );
}

interface AssemblyTreeProps {
  instances: PartInstance[];
  joints: Joint[];
  groundInstanceId?: string;
}

function AssemblyTree({
  instances,
  joints,
  groundInstanceId,
}: AssemblyTreeProps) {
  const renameInstance = useDocumentStore((s) => s.renameInstance);

  const instancesById = useMemo(
    () => new Map(instances.map((i) => [i.id, i])),
    [instances],
  );

  // Build map of child instance -> joint
  const jointByChild = useMemo(
    () => new Map(joints.map((j) => [j.childInstanceId, j])),
    [joints],
  );

  return (
    <div className="space-y-0.5">
      {/* Section header: Instances */}
      <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted/70 px-2 pt-1">
        Instances
      </div>
      {instances.length === 0 ? (
        <div className="px-2 py-2 text-center text-xs text-text-muted/70">
          No instances yet.
        </div>
      ) : (
        instances.map((instance) => (
          <InstanceNode
            key={instance.id}
            instance={instance}
            joint={jointByChild.get(instance.id)}
            isGround={instance.id === groundInstanceId}
            onRename={(id) => {
              const inst = instances.find((i) => i.id === id);
              if (inst) {
                const newName = prompt("Rename instance:", inst.name ?? inst.partDefId);
                if (newName && newName.trim()) {
                  renameInstance(id, newName.trim());
                }
              }
            }}
          />
        ))
      )}

      {/* Section header: Joints (if any) */}
      {joints.length > 0 && (
        <>
          <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted/70 px-2 pt-2">
            Joints
          </div>
          {joints.map((joint) => (
            <JointNode
              key={joint.id}
              joint={joint}
              instancesById={instancesById}
            />
          ))}
        </>
      )}
    </div>
  );
}

export function FeatureTree() {
  const parts = useDocumentStore((s) => s.parts);
  const consumedParts = useDocumentStore((s) => s.consumedParts);
  const document = useDocumentStore((s) => s.document);
  const reorderPart = useDocumentStore((s) => s.reorderPart);
  const featureTreeOpen = useUiStore((s) => s.featureTreeOpen);
  // isOrbiting was used to fade out the floating overlay; no longer needed now
  // that the tree lives in a grid slot. Leaving the selector out avoids a
  // subscribe on every orbit frame.
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [inlineExpandedIds, setInlineExpandedIds] = useState<Set<string>>(new Set());
  const [activeId, setActiveId] = useState<string | null>(null);

  // Sample background luminance to adapt text color

  // Check if this is an assembly document
  const hasInstances = document.instances && document.instances.length > 0;

  // Drag and drop sensors
  const sensors = useSensors(
    // Mouse: 8px movement triggers a drag (lets click/select still work)
    useSensor(MouseSensor, {
      activationConstraint: { distance: 8 },
    }),
    // Touch: long-press 250ms starts a drag so finger-scroll of the tree
    // keeps working without accidentally grabbing a row
    useSensor(TouchSensor, {
      activationConstraint: { delay: 250, tolerance: 5 },
    }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    })
  );

  // Part IDs for sortable context
  const partIds = useMemo(() => parts.map((p) => p.id), [parts]);

  function handleDragStart(event: DragStartEvent) {
    setActiveId(event.active.id as string);
  }

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event;
    setActiveId(null);

    if (!over || active.id === over.id) return;

    const oldIndex = parts.findIndex((p) => p.id === active.id);
    const newIndex = parts.findIndex((p) => p.id === over.id);

    if (oldIndex !== -1 && newIndex !== -1) {
      reorderPart(active.id as string, newIndex);
    }
  }

  // Get the active part for drag overlay
  const activePart = activeId ? parts.find((p) => p.id === activeId) : null;

  useEffect(() => {
    function handleRename() {
      const { selectedPartIds } = useUiStore.getState();
      if (selectedPartIds.size === 1) {
        setRenamingId(Array.from(selectedPartIds)[0]!);
      }
    }
    window.addEventListener("vcad:rename-part", handleRename);
    return () => window.removeEventListener("vcad:rename-part", handleRename);
  }, []);

  function toggleExpanded(id: string) {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  function toggleInlineExpanded(id: string) {
    setInlineExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  // Always show if feature tree is open - scene section is always available
  if (!featureTreeOpen) return null;

  const hasGeometry = hasInstances || parts.length > 0;

  return (
    <div
      className={cn(
        // Sidebar pane — fills its grid slot
        "h-full w-full",
        "flex flex-col",
        "p-2 select-none",
      )}
    >
      <div className="overflow-y-auto scrollbar-thin flex-1">
        <ContextMenu>
          <div className="space-y-0.5">
            {/* Scene row — drill into inspector to edit env / background / lights */}
            <SceneTreeRow />

            {/* Parameters row — document-level named parameters for expression bindings */}
            <ParametersTreeRow />

            {/* Sketch entities / constraints — only while sketching */}
            <SketchTreeSection />

            {/* Empty state when no parts/instances yet */}
            {!hasGeometry && <FeatureTreeEmptyState />}

            {/* Geometry when present */}
            {hasGeometry && (
              <>
                {/* Assembly mode: show instances and joints */}
                {hasInstances ? (
                  <AssemblyTree
                    instances={document.instances!}
                    joints={document.joints ?? []}
                    groundInstanceId={document.groundInstanceId}
                  />
                ) : (
                  /* Parts mode with drag-and-drop */
                  <DndContext
                    sensors={sensors}
                    collisionDetection={closestCenter}
                    onDragStart={handleDragStart}
                    onDragEnd={handleDragEnd}
                  >
                    <SortableContext
                      items={partIds}
                      strategy={verticalListSortingStrategy}
                    >
                      <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted/70 px-2 pt-1">
                        Parts
                      </div>
                      {parts.map((part) => (
                        <SortableTreeNode
                          key={part.id}
                          id={part.id}
                          part={part}
                          depth={0}
                          expandedIds={expandedIds}
                          toggleExpanded={toggleExpanded}
                          consumedParts={consumedParts}
                          renamingId={renamingId}
                          setRenamingId={setRenamingId}
                          inlineExpandedIds={inlineExpandedIds}
                          toggleInlineExpanded={toggleInlineExpanded}
                        />
                      ))}
                    </SortableContext>
                    <DragOverlay dropAnimation={null}>
                      {activePart && <DragPreview part={activePart} />}
                    </DragOverlay>
                  </DndContext>
                )}
              </>
            )}
          </div>
        </ContextMenu>
      </div>
    </div>
  );
}
