import { useState, useEffect, useRef } from "react";
import { LineSegment } from "@phosphor-icons/react/dist/ssr/LineSegment";
import { Rectangle } from "@phosphor-icons/react/dist/ssr/Rectangle";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { ArrowUp } from "@phosphor-icons/react/dist/ssr/ArrowUp";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { ArrowsHorizontal } from "@phosphor-icons/react/dist/ssr/ArrowsHorizontal";
import { ArrowsVertical } from "@phosphor-icons/react/dist/ssr/ArrowsVertical";
import { Ruler } from "@phosphor-icons/react/dist/ssr/Ruler";
import { Equals } from "@phosphor-icons/react/dist/ssr/Equals";
import { GitBranch } from "@phosphor-icons/react/dist/ssr/GitBranch";
import { Play } from "@phosphor-icons/react/dist/ssr/Play";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Spiral } from "@phosphor-icons/react/dist/ssr/Spiral";
import { Stack } from "@phosphor-icons/react/dist/ssr/Stack";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { Crosshair } from "@phosphor-icons/react/dist/ssr/Crosshair";
import { GridFour } from "@phosphor-icons/react/dist/ssr/GridFour";
import { ArrowsClockwise } from "@phosphor-icons/react/dist/ssr/ArrowsClockwise";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Hammer } from "@phosphor-icons/react/dist/ssr/Hammer";
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { Button } from "@/components/ui/button";
import { Tooltip } from "@/components/ui/tooltip";
import { TabDropdown, ToolbarButton } from "@/components/ui/toolbar";
import {
  SKETCH_TAB_COLORS,
  type SketchTab,
} from "@/components/ui/toolbar-constants";
import { cn } from "@/lib/utils";
import { useSketchStore, useDocumentStore, useUiStore, useEngineStore, getSketchPlaneDirections, formatDirection, negateDirection } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";
import { analytics } from "@/lib/analytics";
import type { SketchState, ConstraintTool, SketchPlane } from "@vcad/core";
import type { Vec3, SketchSegment2D } from "@vcad/ir";

/** Confirmation dialog for discarding sketch with segments */
function DiscardConfirmDialog({
  onDiscard,
  onKeepEditing,
}: {
  onDiscard: () => void;
  onKeepEditing: () => void;
}) {
  return (
    <div className="absolute left-1/2 bottom-full mb-2 -translate-x-1/2 border border-border bg-card p-4 shadow-2xl min-w-[240px]">
      <div className="flex flex-col gap-3">
        <div className="flex items-center gap-2 text-amber-500">
          <Warning size={18} weight="fill" />
          <span className="text-sm font-medium">Discard sketch?</span>
        </div>
        <p className="text-xs text-text-muted">
          You have unsaved geometry. This cannot be undone.
        </p>
        <div className="flex gap-2">
          <Button
            variant="default"
            size="sm"
            className="flex-1 bg-red-600 hover:bg-red-700"
            onClick={onDiscard}
          >
            Discard
          </Button>
          <Button variant="ghost" size="sm" className="flex-1" onClick={onKeepEditing}>
            Keep Editing
          </Button>
        </div>
      </div>
    </div>
  );
}

const TOOLS: { tool: SketchState["tool"]; icon: typeof LineSegment; label: string; shortcut: string }[] = [
  { tool: "rectangle", icon: Rectangle, label: "Rectangle", shortcut: "R" },
  { tool: "circle", icon: Circle, label: "Circle", shortcut: "C" },
  { tool: "line", icon: LineSegment, label: "Line", shortcut: "L" },
];

const CONSTRAINT_TOOLS: {
  tool: ConstraintTool;
  icon: typeof LineSegment;
  label: string;
  requires: number;
}[] = [
  { tool: "horizontal", icon: ArrowsHorizontal, label: "Horizontal", requires: 1 },
  { tool: "vertical", icon: ArrowsVertical, label: "Vertical", requires: 1 },
  { tool: "length", icon: Ruler, label: "Length", requires: 1 },
  { tool: "parallel", icon: GitBranch, label: "Parallel", requires: 2 },
  { tool: "equal", icon: Equals, label: "Equal Length", requires: 2 },
];

// Tab definitions for the sketch toolbar
const SKETCH_TABS: { id: SketchTab; label: string; icon: typeof PencilSimple }[] = [
  { id: "draw", label: "Draw", icon: PencilSimple },
  { id: "constrain", label: "Constrain", icon: Hammer },
  { id: "finish", label: "Finish", icon: CheckCircle },
];

function NumberInputDialog({
  label,
  unit,
  defaultValue,
  onSubmit,
  onClose,
}: {
  label: string;
  unit: string;
  defaultValue: number;
  onSubmit: (value: number) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState(defaultValue);

  return (
    <div className="absolute left-1/2 bottom-full mb-2 -translate-x-1/2  border border-border bg-card p-4 shadow-2xl">
      <button
        onClick={onClose}
        className="absolute top-2 right-2 text-text-muted hover:text-text p-1"
        aria-label="Close"
      >
        <X size={14} />
      </button>
      <div className="flex flex-col gap-3">
        <div className="text-xs font-medium text-text">{label}</div>
        <div className="flex items-center gap-2">
          <input
            type="number"
            value={value}
            onChange={(e) => setValue(Number(e.target.value))}
            className="w-24  border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") onSubmit(value);
              if (e.key === "Escape") onClose();
            }}
          />
          <span className="text-xs text-text-muted">{unit}</span>
        </div>
        <div className="flex gap-2">
          <Button
            variant="default"
            size="sm"
            className="flex-1"
            onClick={() => onSubmit(value)}
          >
            Apply
          </Button>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}

interface ExtrudeOptions {
  depth: number;
  twistAngle?: number;  // radians
  scaleEnd?: number;
}

function ExtrudeDialog({
  onExtrude,
  onClose,
  normalDir,
  plane,
  origin,
  segments,
}: {
  onExtrude: (options: ExtrudeOptions) => void;
  onClose: () => void;
  normalDir: string;
  plane: SketchPlane;
  origin: Vec3;
  segments: SketchSegment2D[];
}) {
  const [depth, setDepth] = useState(20);
  const [flip, setFlip] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [twistDeg, setTwistDeg] = useState(0);
  const [scaleEnd, setScaleEnd] = useState(1.0);
  const engine = useEngineStore((s) => s.engine);
  const setPreviewMesh = useEngineStore((s) => s.setPreviewMesh);

  // Animated depth for smooth preview transitions
  const animatedDepthRef = useRef(20);
  const targetDepthRef = useRef(20);
  const rafRef = useRef<number>(0);

  const displayDir = flip ? negateDirection(normalDir) : normalDir;

  // Update target when depth/flip changes
  useEffect(() => {
    targetDepthRef.current = flip ? -depth : depth;
  }, [depth, flip]);

  // Animation loop for smooth preview
  useEffect(() => {
    if (!engine || segments.length === 0) return;

    const { x_dir, y_dir, normal } = getSketchPlaneDirections(plane);
    let lastRenderedDepth = animatedDepthRef.current;

    const animate = () => {
      const target = targetDepthRef.current;
      const current = animatedDepthRef.current;
      const diff = target - current;

      // Lerp toward target (0.15 = smooth, 0.35 = snappy)
      if (Math.abs(diff) > 0.1) {
        animatedDepthRef.current = current + diff * 0.35;
      } else {
        animatedDepthRef.current = target;
      }

      // Only regenerate mesh if depth changed significantly (saves CPU/GPU)
      const depthChange = Math.abs(animatedDepthRef.current - lastRenderedDepth);
      if (depthChange > 0.5) {
        lastRenderedDepth = animatedDepthRef.current;

        const direction = {
          x: normal.x * animatedDepthRef.current,
          y: normal.y * animatedDepthRef.current,
          z: normal.z * animatedDepthRef.current,
        };

        // TODO: Add twist/scale to preview once engine supports it
        const mesh = engine.evaluateExtrudePreview(origin, x_dir, y_dir, segments, direction);
        setPreviewMesh(mesh);
      }

      rafRef.current = requestAnimationFrame(animate);
    };

    // Generate initial preview
    const direction = {
      x: normal.x * animatedDepthRef.current,
      y: normal.y * animatedDepthRef.current,
      z: normal.z * animatedDepthRef.current,
    };
    const mesh = engine.evaluateExtrudePreview(origin, x_dir, y_dir, segments, direction);
    setPreviewMesh(mesh);

    rafRef.current = requestAnimationFrame(animate);

    return () => {
      cancelAnimationFrame(rafRef.current);
      setPreviewMesh(null);
    };
  }, [engine, plane, origin, segments, setPreviewMesh]);

  const handleExtrude = () => {
    const finalDepth = flip ? -depth : depth;
    const twistRad = (twistDeg * Math.PI) / 180;
    const hasTwist = Math.abs(twistDeg) > 0.01;
    const hasScale = Math.abs(scaleEnd - 1.0) > 0.01;

    onExtrude({
      depth: finalDepth,
      twistAngle: hasTwist ? twistRad : undefined,
      scaleEnd: hasScale ? scaleEnd : undefined,
    });
  };

  return (
    <div className="absolute left-1/2 bottom-full mb-2 -translate-x-1/2 border border-border bg-card p-4 shadow-2xl">
      <button
        onClick={onClose}
        className="absolute top-2 right-2 text-text-muted hover:text-text p-1"
        aria-label="Close"
      >
        <X size={14} />
      </button>
      <div className="flex flex-col gap-3">
        <div className="text-xs font-medium text-text">Extrude Depth</div>
        <div className="flex items-center gap-2 text-xs text-text-muted">
          <span>Direction:</span>
          <span className="font-mono text-brand">{displayDir}</span>
        </div>
        <div className="flex items-center gap-2">
          <input
            type="number"
            value={depth}
            onChange={(e) => setDepth(Number(e.target.value))}
            className="w-24 border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") handleExtrude();
              if (e.key === "Escape") onClose();
            }}
          />
          <span className="text-xs text-text-muted">mm</span>
        </div>
        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={flip}
            onChange={(e) => setFlip(e.target.checked)}
            className="accent-brand"
          />
          <span className="text-xs text-text-muted">Flip direction</span>
        </label>

        {/* Advanced options toggle */}
        <button
          className="flex items-center gap-1 text-xs text-text-muted hover:text-text"
          onClick={() => setShowAdvanced(!showAdvanced)}
        >
          <span className={cn("transition-transform", showAdvanced && "rotate-90")}>▶</span>
          <span>Advanced</span>
        </button>

        {/* Advanced options */}
        {showAdvanced && (
          <div className="flex flex-col gap-2 pl-2 border-l-2 border-border">
            <div className="flex items-center gap-2">
              <span className="text-xs text-text-muted w-12">Twist:</span>
              <input
                type="number"
                value={twistDeg}
                onChange={(e) => setTwistDeg(Number(e.target.value))}
                className="w-16 border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
                step="15"
              />
              <span className="text-xs text-text-muted">°</span>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs text-text-muted w-12">Scale:</span>
              <input
                type="number"
                value={scaleEnd}
                onChange={(e) => setScaleEnd(Number(e.target.value))}
                className="w-16 border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
                step="0.1"
                min="0.1"
                max="3.0"
              />
            </div>
          </div>
        )}

        <div className="flex gap-2">
          <Button
            variant="default"
            size="sm"
            className="flex-1"
            onClick={handleExtrude}
          >
            Extrude
          </Button>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}

function RevolveDialog({
  onRevolve,
  onClose,
  plane,
  origin,
  segments,
}: {
  onRevolve: (angleDeg: number, flip: boolean) => void;
  onClose: () => void;
  plane: SketchPlane;
  origin: Vec3;
  segments: SketchSegment2D[];
}) {
  const [angle, setAngle] = useState(360);
  const [flip, setFlip] = useState(false);
  const engine = useEngineStore((s) => s.engine);
  const setPreviewMesh = useEngineStore((s) => s.setPreviewMesh);

  // Animated angle for smooth preview transitions
  const animatedAngleRef = useRef(360);
  const targetAngleRef = useRef(360);
  const rafRef = useRef<number>(0);

  // Update target when angle changes
  useEffect(() => {
    targetAngleRef.current = angle;
  }, [angle]);

  // Animation loop for smooth preview
  useEffect(() => {
    if (!engine || segments.length === 0) return;

    const { x_dir, y_dir } = getSketchPlaneDirections(plane);
    let lastRenderedAngle = animatedAngleRef.current;

    // Compute axis origin (edge of sketch bounding box) and direction
    // For revolve, the axis is along the local X axis of the sketch plane, at the sketch origin
    const axisOrigin = origin;
    const axisDir = flip ? { x: -x_dir.x, y: -x_dir.y, z: -x_dir.z } : x_dir;

    const animate = () => {
      const target = targetAngleRef.current;
      const current = animatedAngleRef.current;
      const diff = target - current;

      // Lerp toward target
      if (Math.abs(diff) > 0.5) {
        animatedAngleRef.current = current + diff * 0.35;
      } else {
        animatedAngleRef.current = target;
      }

      // Only regenerate mesh if angle changed significantly
      const angleChange = Math.abs(animatedAngleRef.current - lastRenderedAngle);
      if (angleChange > 2) {
        lastRenderedAngle = animatedAngleRef.current;

        const mesh = engine.evaluateRevolvePreview(
          origin,
          x_dir,
          y_dir,
          segments,
          axisOrigin,
          axisDir,
          animatedAngleRef.current
        );
        setPreviewMesh(mesh);
      }

      rafRef.current = requestAnimationFrame(animate);
    };

    // Generate initial preview
    const mesh = engine.evaluateRevolvePreview(
      origin,
      x_dir,
      y_dir,
      segments,
      axisOrigin,
      axisDir,
      animatedAngleRef.current
    );
    setPreviewMesh(mesh);

    rafRef.current = requestAnimationFrame(animate);

    return () => {
      cancelAnimationFrame(rafRef.current);
      setPreviewMesh(null);
    };
  }, [engine, plane, origin, segments, flip, setPreviewMesh]);

  return (
    <div className="absolute left-1/2 bottom-full mb-2 -translate-x-1/2 border border-border bg-card p-4 shadow-2xl">
      <button
        onClick={onClose}
        className="absolute top-2 right-2 text-text-muted hover:text-text p-1"
        aria-label="Close"
      >
        <X size={14} />
      </button>
      <div className="flex flex-col gap-3">
        <div className="text-xs font-medium text-text">Revolve Angle</div>
        <div className="flex items-center gap-2">
          <input
            type="number"
            value={angle}
            onChange={(e) => setAngle(Number(e.target.value))}
            className="w-24 border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") onRevolve(angle, flip);
              if (e.key === "Escape") onClose();
            }}
          />
          <span className="text-xs text-text-muted">°</span>
        </div>
        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={flip}
            onChange={(e) => setFlip(e.target.checked)}
            className="accent-brand"
          />
          <span className="text-xs text-text-muted">Flip axis direction</span>
        </label>
        <div className="flex gap-2">
          <Button
            variant="default"
            size="sm"
            className="flex-1"
            onClick={() => onRevolve(angle, flip)}
          >
            Revolve
          </Button>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}

function SweepDialog({
  onSweep,
  onClose,
}: {
  onSweep: (params: { type: "line" | "helix"; height: number; radius?: number; turns?: number }) => void;
  onClose: () => void;
}) {
  const [pathType, setPathType] = useState<"line" | "helix">("line");
  const [height, setHeight] = useState(20);
  const [radius, setRadius] = useState(10);
  const [turns, setTurns] = useState(2);

  return (
    <div className="fixed left-1/2 bottom-20 -translate-x-1/2 z-50 border border-border bg-card p-4 shadow-2xl">
      <button
        onClick={onClose}
        className="absolute top-2 right-2 text-text-muted hover:text-text p-1"
        aria-label="Close"
      >
        <X size={14} />
      </button>
      <div className="flex flex-col gap-3">
        <div className="text-xs font-medium text-text">Sweep Path</div>
        <div className="flex gap-2">
          <button
            onClick={() => setPathType("line")}
            className={`px-3 py-1  text-xs ${pathType === "line" ? "bg-brand text-white" : "bg-surface text-text hover:bg-border/30"}`}
          >
            Line
          </button>
          <button
            onClick={() => setPathType("helix")}
            className={`px-3 py-1  text-xs ${pathType === "helix" ? "bg-brand text-white" : "bg-surface text-text hover:bg-border/30"}`}
          >
            Helix
          </button>
        </div>

        <div className="flex items-center gap-2">
          <span className="text-xs text-text-muted w-12">Height</span>
          <input
            type="number"
            value={height}
            onChange={(e) => setHeight(Number(e.target.value))}
            className="w-20  border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
          />
          <span className="text-xs text-text-muted">mm</span>
        </div>

        {pathType === "helix" && (
          <>
            <div className="flex items-center gap-2">
              <span className="text-xs text-text-muted w-12">Radius</span>
              <input
                type="number"
                value={radius}
                onChange={(e) => setRadius(Number(e.target.value))}
                className="w-20  border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
              />
              <span className="text-xs text-text-muted">mm</span>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-xs text-text-muted w-12">Turns</span>
              <input
                type="number"
                value={turns}
                onChange={(e) => setTurns(Number(e.target.value))}
                className="w-20  border border-border bg-bg px-2 py-1 text-sm text-text outline-none focus:border-brand"
              />
            </div>
          </>
        )}

        <div className="flex gap-2">
          <Button
            variant="default"
            size="sm"
            className="flex-1"
            onClick={() => onSweep({ type: pathType, height, radius, turns })}
          >
            Create
          </Button>
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}

function LoftHeightDialog({
  onSetHeight,
  onClose,
  profileCount,
}: {
  onSetHeight: (height: number) => void;
  onClose: () => void;
  profileCount: number;
}) {
  return (
    <NumberInputDialog
      label={`Profile ${profileCount + 1} Z offset`}
      unit="mm"
      defaultValue={(profileCount + 1) * 10}
      onSubmit={onSetHeight}
      onClose={onClose}
    />
  );
}

export function SketchToolbar() {
  const [showExtrudeDialog, setShowExtrudeDialog] = useState(false);
  const [showRevolveDialog, setShowRevolveDialog] = useState(false);
  const [showLengthDialog, setShowLengthDialog] = useState(false);
  const [showSweepDialog, setShowSweepDialog] = useState(false);
  const [showLoftHeightDialog, setShowLoftHeightDialog] = useState(false);

  const active = useSketchStore((s) => s.active);
  const plane = useSketchStore((s) => s.plane);
  const origin = useSketchStore((s) => s.origin);
  const segments = useSketchStore((s) => s.segments);
  const constraints = useSketchStore((s) => s.constraints);
  const tool = useSketchStore((s) => s.tool);
  const constraintTool = useSketchStore((s) => s.constraintTool);
  const selectedSegments = useSketchStore((s) => s.selectedSegments);
  const constraintStatus = useSketchStore((s) => s.constraintStatus);
  const loftMode = useSketchStore((s) => s.loftMode);
  const profiles = useSketchStore((s) => s.profiles);
  const pendingExit = useSketchStore((s) => s.pendingExit);
  const setTool = useSketchStore((s) => s.setTool);
  const setConstraintTool = useSketchStore((s) => s.setConstraintTool);
  const exitSketchMode = useSketchStore((s) => s.exitSketchMode);
  const requestExit = useSketchStore((s) => s.requestExit);
  const confirmExit = useSketchStore((s) => s.confirmExit);
  const cancelExit = useSketchStore((s) => s.cancelExit);
  const clearSketch = useSketchStore((s) => s.clearSketch);
  const applyHorizontal = useSketchStore((s) => s.applyHorizontal);
  const applyVertical = useSketchStore((s) => s.applyVertical);
  const applyLength = useSketchStore((s) => s.applyLength);
  const applyParallel = useSketchStore((s) => s.applyParallel);
  const applyEqual = useSketchStore((s) => s.applyEqual);
  const solveSketch = useSketchStore((s) => s.solveSketch);
  const clearSelection = useSketchStore((s) => s.clearSelection);
  const saveProfile = useSketchStore((s) => s.saveProfile);
  const clearForNextProfile = useSketchStore((s) => s.clearForNextProfile);
  const exitLoftMode = useSketchStore((s) => s.exitLoftMode);

  const addExtrude = useDocumentStore((s) => s.addExtrude);
  const addRevolve = useDocumentStore((s) => s.addRevolve);
  const addSweep = useDocumentStore((s) => s.addSweep);
  const addLoft = useDocumentStore((s) => s.addLoft);
  const select = useUiStore((s) => s.select);
  const gridSnap = useUiStore((s) => s.gridSnap);
  const pointSnap = useUiStore((s) => s.pointSnap);
  const toggleGridSnap = useUiStore((s) => s.toggleGridSnap);
  const togglePointSnap = useUiStore((s) => s.togglePointSnap);
  const isOrbiting = useUiStore((s) => s.isOrbiting);
  const addToast = useNotificationStore((s) => s.addToast);

  // Listen for quick extrude keyboard shortcut
  useEffect(() => {
    const handleQuickExtrude = () => {
      if (active && segments.length > 0 && !loftMode) {
        setShowExtrudeDialog(true);
      }
    };
    window.addEventListener("vcad:sketch-extrude", handleQuickExtrude);
    return () => window.removeEventListener("vcad:sketch-extrude", handleQuickExtrude);
  }, [active, segments.length, loftMode]);

  if (!active) return null;

  const hasSegments = segments.length > 0;
  const hasConstraints = constraints.length > 0;
  const isConstraintMode = constraintTool !== "none";

  function handleConstraintToolClick(ct: ConstraintTool) {
    if (constraintTool === ct) {
      // Toggle off
      clearSelection();
    } else {
      setConstraintTool(ct);
    }
  }

  function handleApplyConstraint() {
    switch (constraintTool) {
      case "horizontal":
        applyHorizontal();
        break;
      case "vertical":
        applyVertical();
        break;
      case "length":
        if (selectedSegments.length === 1) {
          setShowLengthDialog(true);
        }
        break;
      case "parallel":
        applyParallel();
        break;
      case "equal":
        applyEqual();
        break;
    }
  }

  // Check if we have enough segments selected for current constraint tool
  const currentToolReq = CONSTRAINT_TOOLS.find((t) => t.tool === constraintTool)?.requires ?? 0;
  const canApplyConstraint = selectedSegments.length === currentToolReq;

  function handleExtrude(options: ExtrudeOptions) {
    if (!hasSegments) return;

    // Determine extrusion direction based on plane normal
    const { normal } = getSketchPlaneDirections(plane);
    const direction = {
      x: normal.x * options.depth,
      y: normal.y * options.depth,
      z: normal.z * options.depth,
    };

    const partId = addExtrude(plane, origin, segments, direction, {
      twist_angle: options.twistAngle,
      scale_end: options.scaleEnd,
    });
    if (partId) {
      select(partId);
      analytics.extrudeApplied();
      analytics.sketchCompleted(constraints.length);
      const label = options.twistAngle || options.scaleEnd
        ? "Created Twisted Extrude"
        : "Created Extrude";
      addToast(label, "success");
    }
    exitSketchMode();
    setShowExtrudeDialog(false);
  }

  function handleRevolve(angleDeg: number, flip: boolean) {
    if (!hasSegments) return;

    const { x_dir } = getSketchPlaneDirections(plane);
    // Axis is along local X direction of sketch plane
    const axisOrigin = origin;
    const axisDir = flip ? { x: -x_dir.x, y: -x_dir.y, z: -x_dir.z } : x_dir;

    const partId = addRevolve(plane, origin, segments, axisOrigin, axisDir, angleDeg);
    if (partId) {
      select(partId);
      addToast("Created Revolve", "success");
    }
    exitSketchMode();
    setShowRevolveDialog(false);
  }

  function handleSweep(params: { type: "line" | "helix"; height: number; radius?: number; turns?: number }) {
    if (!hasSegments) return;

    // Create path based on type
    const { normal } = getSketchPlaneDirections(plane);
    const path = (() => {
      if (params.type === "line") {
        // Line path along the normal direction of the sketch plane
        const start = origin;
        const end = {
          x: origin.x + normal.x * params.height,
          y: origin.y + normal.y * params.height,
          z: origin.z + normal.z * params.height,
        };
        return { type: "Line" as const, start, end };
      } else {
        // Helix path
        return {
          type: "Helix" as const,
          radius: params.radius ?? 10,
          pitch: params.height / (params.turns ?? 2),
          height: params.height,
          turns: params.turns ?? 2,
        };
      }
    })();

    const partId = addSweep(plane, origin, segments, path);
    if (partId) {
      select(partId);
      addToast("Created Sweep", "success");
    }
    exitSketchMode();
    setShowSweepDialog(false);
  }

  function handleAddProfile(zOffset: number) {
    // Save current profile and prepare for next
    saveProfile();

    // Calculate new origin with offset along plane normal
    const { normal } = getSketchPlaneDirections(plane);
    const newOrigin = {
      x: origin.x + normal.x * zOffset,
      y: origin.y + normal.y * zOffset,
      z: origin.z + normal.z * zOffset,
    };

    clearForNextProfile(newOrigin);
    setShowLoftHeightDialog(false);
  }

  function handleCreateLoft() {
    const allProfiles = exitLoftMode();
    if (allProfiles && allProfiles.length >= 2) {
      const partId = addLoft(
        allProfiles.map((p) => ({
          plane: p.plane,
          origin: p.origin,
          segments: p.segments,
        })),
      );
      if (partId) {
        select(partId);
        addToast("Created Loft", "success");
      }
    }
  }

  function handleCancel() {
    if (loftMode) {
      exitLoftMode();
      addToast("Sketch cancelled", "info");
    } else {
      // Use requestExit to check if confirmation is needed
      const exited = requestExit();
      if (exited) {
        addToast("Sketch cancelled", "info");
      }
      // If not exited, pendingExit will be set and confirmation dialog will show
    }
  }

  function handleDiscardSketch() {
    confirmExit();
    addToast("Sketch discarded", "info");
  }

  function handleKeepEditing() {
    cancelExit();
  }

  // Render content for Draw tab
  const renderDrawContent = () => (
    <>
      {TOOLS.map(({ tool: t, icon: Icon, label, shortcut }) => (
        <ToolbarButton
          key={t}
          tooltip={`${label} (${shortcut})`}
          active={tool === t && !isConstraintMode}
          onClick={() => {
            clearSelection();
            setTool(t);
          }}
          iconColor={SKETCH_TAB_COLORS.draw}
        >
          <Icon size={20} />
          <span className="absolute bottom-0.5 right-0.5 text-[8px] font-mono text-text-muted/50">
            {shortcut}
          </span>
        </ToolbarButton>
      ))}
      <ToolbarButton
        tooltip="Toggle point snap (P)"
        active={pointSnap}
        onClick={togglePointSnap}
        iconColor={pointSnap ? "text-green-400" : undefined}
      >
        <Crosshair size={20} />
        <span className="absolute bottom-0.5 right-0.5 text-[8px] font-mono text-text-muted/50">
          P
        </span>
      </ToolbarButton>
      <ToolbarButton
        tooltip="Toggle grid snap (G)"
        active={gridSnap}
        onClick={toggleGridSnap}
        iconColor={gridSnap ? "text-cyan-400" : undefined}
      >
        <GridFour size={20} />
        <span className="absolute bottom-0.5 right-0.5 text-[8px] font-mono text-text-muted/50">
          G
        </span>
      </ToolbarButton>
    </>
  );

  // Render content for Constrain tab
  const renderConstrainContent = () => (
    <>
      {CONSTRAINT_TOOLS.map(({ tool: ct, icon: Icon, label, requires }) => (
        <ToolbarButton
          key={ct}
          tooltip={`${label} (select ${requires} segment${requires > 1 ? "s" : ""})`}
          active={constraintTool === ct}
          onClick={() => handleConstraintToolClick(ct)}
          disabled={!hasSegments}
          iconColor={SKETCH_TAB_COLORS.constrain}
        >
          <Icon size={20} />
        </ToolbarButton>
      ))}
      {/* Apply constraint button (shows when ready) */}
      {isConstraintMode && canApplyConstraint && (
        <ToolbarButton
          tooltip="Apply constraint"
          onClick={handleApplyConstraint}
          className="bg-green-600 hover:bg-green-700"
          iconColor="text-white"
        >
          <Play size={20} weight="fill" />
        </ToolbarButton>
      )}
      {/* Solve button */}
      <ToolbarButton
        tooltip="Solve constraints"
        onClick={solveSketch}
        disabled={!hasConstraints}
        className={constraintStatus === "error" ? "bg-amber-600 hover:bg-amber-700" : undefined}
        iconColor={SKETCH_TAB_COLORS.constrain}
      >
        <Play size={20} />
      </ToolbarButton>
      {/* Constraint status indicator */}
      {hasSegments && (
        <Tooltip
          content={
            constraintStatus === "under"
              ? "Under-constrained (add more constraints)"
              : constraintStatus === "solved"
                ? "Fully constrained"
                : constraintStatus === "over"
                  ? "Over-constrained (remove constraints)"
                  : "Constraints conflict"
          }
        >
          <div className="flex items-center gap-1 px-2">
            <div
              className={cn(
                "w-2 h-2 rounded-full",
                constraintStatus === "under" && "bg-yellow-500",
                constraintStatus === "solved" && "bg-green-500",
                constraintStatus === "over" && "bg-orange-500",
                constraintStatus === "error" && "bg-red-500"
              )}
            />
            <span
              className={cn(
                "text-xs",
                constraintStatus === "under" && "text-yellow-500",
                constraintStatus === "solved" && "text-green-500",
                constraintStatus === "over" && "text-orange-500",
                constraintStatus === "error" && "text-red-500"
              )}
            >
              {constraints.length}
            </span>
          </div>
        </Tooltip>
      )}
    </>
  );

  // Render content for Finish tab (changes in loft mode)
  const renderFinishContent = () => {
    if (loftMode) {
      return (
        <>
          <div className="px-2 text-xs text-brand font-medium">
            Profile {profiles.length + (hasSegments ? 1 : 0)}
          </div>
          <ToolbarButton
            tooltip="Save profile & add next"
            onClick={() => setShowLoftHeightDialog(true)}
            disabled={!hasSegments}
            iconColor={SKETCH_TAB_COLORS.finish}
          >
            <Plus size={20} />
          </ToolbarButton>
          <ToolbarButton
            tooltip="Create Loft"
            onClick={handleCreateLoft}
            disabled={profiles.length < 1 || !hasSegments}
            className={profiles.length >= 1 && hasSegments ? "bg-green-600 hover:bg-green-700" : undefined}
            iconColor={profiles.length >= 1 && hasSegments ? "text-white" : SKETCH_TAB_COLORS.finish}
          >
            <Stack size={20} />
          </ToolbarButton>
          <ToolbarButton
            tooltip="Cancel sketch (Esc)"
            onClick={handleCancel}
            iconColor={SKETCH_TAB_COLORS.finish}
          >
            <X size={20} />
          </ToolbarButton>
        </>
      );
    }

    return (
      <>
        <ToolbarButton
          tooltip="Extrude sketch (E)"
          onClick={() => setShowExtrudeDialog(true)}
          disabled={!hasSegments}
          iconColor={SKETCH_TAB_COLORS.finish}
        >
          <ArrowUp size={20} />
          <span className="absolute bottom-0.5 right-0.5 text-[8px] font-mono text-text-muted/50">
            E
          </span>
        </ToolbarButton>
        <ToolbarButton
          tooltip="Revolve sketch"
          onClick={() => setShowRevolveDialog(true)}
          disabled={!hasSegments}
          iconColor={SKETCH_TAB_COLORS.finish}
        >
          <ArrowsClockwise size={20} />
        </ToolbarButton>
        <ToolbarButton
          tooltip="Sweep sketch"
          onClick={() => setShowSweepDialog(true)}
          disabled={!hasSegments}
          iconColor={SKETCH_TAB_COLORS.finish}
        >
          <Spiral size={20} />
        </ToolbarButton>
        <ToolbarButton
          tooltip="Clear sketch"
          onClick={clearSketch}
          disabled={!hasSegments}
          iconColor={SKETCH_TAB_COLORS.finish}
        >
          <Trash size={20} />
        </ToolbarButton>
        <ToolbarButton
          tooltip="Cancel sketch (Esc)"
          onClick={handleCancel}
          iconColor={SKETCH_TAB_COLORS.finish}
        >
          <X size={20} />
          <span className="absolute bottom-0.5 right-0.5 text-[8px] font-mono text-text-muted/50">
            Esc
          </span>
        </ToolbarButton>
      </>
    );
  };

  return (
    <>
      {/* Bottom toolbar with tabs */}
      <div className={cn(
        "fixed left-1/2 bottom-4 sm:bottom-6 z-30 -translate-x-1/2",
        "max-w-[calc(100vw-16px)] sm:max-w-none",
        "transition-opacity duration-200",
        isOrbiting && "opacity-0 pointer-events-none"
      )}>
        <div className={cn(
          "relative flex items-center gap-0.5",
          "bg-surface/95 backdrop-blur-sm",
          "overflow-x-auto scrollbar-none"
        )}>
          {/* Tab dropdowns */}
          {SKETCH_TABS.map(({ id, label, icon }) => (
            <TabDropdown
              key={id}
              id={id}
              label={label}
              icon={icon}
              colors={SKETCH_TAB_COLORS}
            >
              {id === "draw" && renderDrawContent()}
              {id === "constrain" && renderConstrainContent()}
              {id === "finish" && renderFinishContent()}
            </TabDropdown>
          ))}
        </div>

        {/* Dialogs positioned above toolbar */}
        {showExtrudeDialog && (
          <ExtrudeDialog
            onExtrude={handleExtrude}
            onClose={() => setShowExtrudeDialog(false)}
            normalDir={formatDirection(getSketchPlaneDirections(plane).normal)}
            plane={plane}
            origin={origin}
            segments={segments}
          />
        )}

        {showRevolveDialog && (
          <RevolveDialog
            onRevolve={handleRevolve}
            onClose={() => setShowRevolveDialog(false)}
            plane={plane}
            origin={origin}
            segments={segments}
          />
        )}

        {showSweepDialog && (
          <SweepDialog
            onSweep={handleSweep}
            onClose={() => setShowSweepDialog(false)}
          />
        )}

        {showLoftHeightDialog && (
          <LoftHeightDialog
            onSetHeight={handleAddProfile}
            onClose={() => setShowLoftHeightDialog(false)}
            profileCount={profiles.length}
          />
        )}

        {showLengthDialog && (
          <NumberInputDialog
            label="Length"
            unit="mm"
            defaultValue={10}
            onSubmit={(len) => {
              applyLength(len);
              setShowLengthDialog(false);
            }}
            onClose={() => setShowLengthDialog(false)}
          />
        )}

        {pendingExit && (
          <DiscardConfirmDialog
            onDiscard={handleDiscardSketch}
            onKeepEditing={handleKeepEditing}
          />
        )}
      </div>
    </>
  );
}
