import { useEffect, useMemo } from "react";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { ScrubInput } from "@/components/ui/scrub-input";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { Tooltip } from "@/components/ui/tooltip";
import {
  useSketchStore,
  useDocumentStore,
  useUiStore,
  useEngineStore,
  getSketchPlaneDirections,
  formatDirection,
  negateDirection,
  type PendingOperation,
  type SketchStore,
} from "@vcad/core";
import type { SketchSegment2D, SketchConstraint } from "@vcad/ir";
import { useNotificationStore } from "@/stores/notification-store";
import { analytics } from "@/lib/analytics";

/* -------------------------------------------------------------------------- */
/*  Layout primitives — match PropertyPanel's existing visual language        */
/* -------------------------------------------------------------------------- */

function SectionHeader({ children, tooltip }: { children: React.ReactNode; tooltip?: string }) {
  return (
    <div
      className="text-[10px] font-medium uppercase tracking-wider text-text-muted pt-2 pb-1"
      title={tooltip}
    >
      {children}
    </div>
  );
}

function StatusPill({ tone, children }: { tone: "info" | "warn" | "ok" | "err"; children: React.ReactNode }) {
  const colors = {
    info: "text-text-muted bg-hover/40",
    warn: "text-amber-300 bg-amber-500/10",
    ok: "text-emerald-400 bg-emerald-500/10",
    err: "text-red-400 bg-red-500/10",
  } as const;
  return (
    <span className={cn("inline-block px-1.5 py-0.5 text-[10px] tracking-wide", colors[tone])}>
      {children}
    </span>
  );
}

/* -------------------------------------------------------------------------- */
/*  Operation params — Extrude / Revolve / Sweep / Loft                       */
/* -------------------------------------------------------------------------- */

function FlipCheckbox({
  checked,
  onChange,
  label = "Flip",
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
}) {
  return (
    <label className="flex items-center gap-1 cursor-pointer text-[11px] text-text-muted">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="accent-brand h-3 w-3"
      />
      {label}
    </label>
  );
}

function ExtrudeParams({ op }: { op: Extract<PendingOperation, { kind: "extrude" }> }) {
  const updateOperation = useSketchStore((s) => s.updateOperation);
  const plane = useSketchStore((s) => s.plane);
  const normalDir = formatDirection(getSketchPlaneDirections(plane).normal);
  const displayDir = op.flip ? negateDirection(normalDir) : normalDir;

  return (
    <div className="space-y-1.5">
      <ScrubInput
        label="D"
        tooltip="Depth (mm)"
        value={op.depth}
        min={0.1}
        step={0.5}
        onChange={(v) => updateOperation({ depth: Math.max(0.1, v) } as Partial<PendingOperation>)}
        unit="mm"
      />
      <div className="flex items-center justify-between gap-2 pl-5">
        <FlipCheckbox checked={op.flip} onChange={(v) => updateOperation({ flip: v } as Partial<PendingOperation>)} />
        <span className="text-[10px] text-text-muted">
          dir <span className="font-mono text-brand">{displayDir}</span>
        </span>
      </div>
      <ScrubInput
        label="T"
        tooltip="Twist angle along extrusion (°)"
        value={op.twistDeg}
        step={5}
        onChange={(v) => updateOperation({ twistDeg: v } as Partial<PendingOperation>)}
        unit="°"
      />
      <ScrubInput
        label="S"
        tooltip="Scale at end of extrusion (1.0 = no taper)"
        value={op.scaleEnd}
        min={0.1}
        step={0.1}
        onChange={(v) => updateOperation({ scaleEnd: Math.max(0.1, v) } as Partial<PendingOperation>)}
      />
    </div>
  );
}

function RevolveParams({ op }: { op: Extract<PendingOperation, { kind: "revolve" }> }) {
  const updateOperation = useSketchStore((s) => s.updateOperation);
  return (
    <div className="space-y-1.5">
      <ScrubInput
        label="A"
        tooltip="Sweep angle (°)"
        value={op.angleDeg}
        step={5}
        onChange={(v) => updateOperation({ angleDeg: v } as Partial<PendingOperation>)}
        unit="°"
      />
      <div className="pl-5">
        <FlipCheckbox
          label="Flip axis"
          checked={op.flip}
          onChange={(v) => updateOperation({ flip: v } as Partial<PendingOperation>)}
        />
      </div>
    </div>
  );
}

function SweepParams({ op }: { op: Extract<PendingOperation, { kind: "sweep" }> }) {
  const updateOperation = useSketchStore((s) => s.updateOperation);
  return (
    <div className="space-y-1.5">
      <div className="flex gap-1">
        {(["line", "helix"] as const).map((kind) => (
          <button
            key={kind}
            type="button"
            onClick={() => updateOperation({ pathType: kind } as Partial<PendingOperation>)}
            className={cn(
              "flex-1 px-2 py-0.5 text-[11px] capitalize",
              op.pathType === kind ? "bg-brand text-white" : "bg-hover/40 text-text hover:bg-hover/60",
            )}
          >
            {kind}
          </button>
        ))}
      </div>
      <ScrubInput
        label="H"
        tooltip="Path height (mm)"
        value={op.height}
        min={0.1}
        step={0.5}
        onChange={(v) => updateOperation({ height: Math.max(0.1, v) } as Partial<PendingOperation>)}
        unit="mm"
      />
      {op.pathType === "helix" && (
        <>
          <ScrubInput
            label="R"
            tooltip="Helix radius (mm)"
            value={op.radius}
            min={0.1}
            step={0.5}
            onChange={(v) => updateOperation({ radius: Math.max(0.1, v) } as Partial<PendingOperation>)}
            unit="mm"
          />
          <ScrubInput
            label="N"
            tooltip="Helix turns"
            value={op.turns}
            min={0.25}
            step={0.25}
            onChange={(v) => updateOperation({ turns: Math.max(0.25, v) } as Partial<PendingOperation>)}
          />
        </>
      )}
    </div>
  );
}

function LoftParams({
  op,
  profileCount,
  hasSegments,
  onSaveProfile,
}: {
  op: Extract<PendingOperation, { kind: "loft" }>;
  profileCount: number;
  hasSegments: boolean;
  onSaveProfile: () => void;
}) {
  const updateOperation = useSketchStore((s) => s.updateOperation);

  return (
    <div className="space-y-2">
      <SectionHeader>Loft</SectionHeader>
      <div className="text-xs text-text-muted">
        Profile {profileCount + (hasSegments ? 1 : 0)} of N
      </div>
      <Button
        variant="default"
        size="sm"
        className="w-full"
        onClick={onSaveProfile}
        disabled={!hasSegments}
      >
        <Plus size={14} className="mr-1" />
        Save profile & add next
      </Button>
      <label className="flex items-center gap-2 cursor-pointer text-xs text-text-muted">
        <input
          type="checkbox"
          checked={op.closed}
          onChange={(e) => updateOperation({ closed: e.target.checked } as Partial<PendingOperation>)}
          className="accent-brand"
        />
        Closed loft
      </label>
    </div>
  );
}

function OperationSection({
  op,
  hasSegments,
  profileCount,
  onSaveProfile,
}: {
  op: PendingOperation;
  hasSegments: boolean;
  profileCount: number;
  onSaveProfile: () => void;
}) {
  // No header — the green ✓ button at the top of the SKETCH card already names
  // the operation, repeating "Awaiting Extrude" here is just visual noise.
  return (
    <div className="border-b border-border/40 pb-2 mb-1">
      {op.kind === "extrude" && <ExtrudeParams op={op} />}
      {op.kind === "revolve" && <RevolveParams op={op} />}
      {op.kind === "sweep" && <SweepParams op={op} />}
      {op.kind === "loft" && (
        <LoftParams
          op={op}
          profileCount={profileCount}
          hasSegments={hasSegments}
          onSaveProfile={onSaveProfile}
        />
      )}
    </div>
  );
}

/* -------------------------------------------------------------------------- */
/*  Tool / entity / constraint context                                        */
/* -------------------------------------------------------------------------- */

const TOOL_HELP: Record<SketchStore["tool"], { label: string; hint: string }> = {
  line: { label: "Line", hint: "Click to place points. Each consecutive pair forms a line segment." },
  rectangle: { label: "Rectangle", hint: "Click two corners to draw an axis-aligned rectangle." },
  circle: { label: "Circle", hint: "Click center, then a point on the edge to set radius." },
};

function ToolSection({
  tool,
  constraintTool,
  selectedSegments,
}: {
  tool: SketchStore["tool"];
  constraintTool: SketchStore["constraintTool"];
  selectedSegments: number[];
}) {
  if (constraintTool !== "none") {
    const required: Partial<Record<Exclude<SketchStore["constraintTool"], "none">, number>> = {
      horizontal: 1,
      vertical: 1,
      length: 1,
      parallel: 2,
      perpendicular: 2,
      equal: 2,
      distance: 2,
      fixed: 1,
      coincident: 2,
    };
    const need = required[constraintTool] ?? 1;
    const have = selectedSegments.length;
    const ready = have === need;
    return (
      <div className="flex items-center justify-between gap-2 text-[11px]">
        <span className="capitalize text-text">{constraintTool}</span>
        <StatusPill tone={ready ? "ok" : "warn"}>
          {have}/{need} selected
        </StatusPill>
      </div>
    );
  }

  const help = TOOL_HELP[tool];
  return (
    <div className="text-[11px] text-text-muted">
      <span className="text-text">{help.label}</span> — {help.hint.toLowerCase().replace(/\.$/, "")}
    </div>
  );
}

function EntitySection({
  segment,
  index,
  onClear,
}: {
  segment: SketchSegment2D;
  index: number;
  onClear: () => void;
}) {
  const length = useMemo(() => {
    if (segment.type === "Line") {
      const dx = segment.end.x - segment.start.x;
      const dy = segment.end.y - segment.start.y;
      return Math.hypot(dx, dy);
    }
    if (segment.type === "Arc") {
      const r = Math.hypot(segment.start.x - segment.center.x, segment.start.y - segment.center.y);
      const sa = Math.atan2(segment.start.y - segment.center.y, segment.start.x - segment.center.x);
      const ea = Math.atan2(segment.end.y - segment.center.y, segment.end.x - segment.center.x);
      let dtheta = ea - sa;
      if (segment.ccw && dtheta < 0) dtheta += 2 * Math.PI;
      if (!segment.ccw && dtheta > 0) dtheta -= 2 * Math.PI;
      return Math.abs(dtheta) * r;
    }
    return 0;
  }, [segment]);

  return (
    <div className="flex items-center justify-between gap-2 text-[11px]">
      <span className="text-text">
        {segment.type} {index + 1}
        <span className="text-text-muted ml-2 tabular-nums">
          {length.toFixed(1)}mm
        </span>
      </span>
      <button
        type="button"
        onClick={onClear}
        className="text-text-muted hover:text-text text-[10px]"
        aria-label="Clear selection"
      >
        clear
      </button>
    </div>
  );
}

function ConstraintSection({
  constraint,
  index,
  onRemove,
}: {
  constraint: SketchConstraint;
  index: number;
  onRemove: (index: number) => void;
}) {
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-[11px]">
        <span className="text-text">{constraint.type} {index + 1}</span>
        <Tooltip content="Delete constraint" side="top">
          <button
            type="button"
            onClick={() => onRemove(index)}
            className="text-text-muted hover:text-red-400 flex items-center gap-1"
          >
            <Trash size={10} />
            delete
          </button>
        </Tooltip>
      </div>
      <pre className="text-[9px] text-text-muted whitespace-pre-wrap leading-tight">
        {JSON.stringify(constraint, null, 2)}
      </pre>
    </div>
  );
}

/* -------------------------------------------------------------------------- */
/*  Top-level sketch panel                                                    */
/* -------------------------------------------------------------------------- */

export function SketchPropertyPanel() {
  const active = useSketchStore((s) => s.active);
  const tool = useSketchStore((s) => s.tool);
  const constraintTool = useSketchStore((s) => s.constraintTool);
  const segments = useSketchStore((s) => s.segments);
  const constraints = useSketchStore((s) => s.constraints);
  const selectedSegments = useSketchStore((s) => s.selectedSegments);
  const selectedConstraintIndex = useSketchStore((s) => s.selectedConstraintIndex);
  const pendingOperation = useSketchStore((s) => s.pendingOperation);
  const profiles = useSketchStore((s) => s.profiles);
  const plane = useSketchStore((s) => s.plane);
  const origin = useSketchStore((s) => s.origin);
  const loftMode = useSketchStore((s) => s.loftMode);

  const clearSelection = useSketchStore((s) => s.clearSelection);
  const removeConstraint = useSketchStore((s) => s.removeConstraint);
  const setSelectedConstraint = useSketchStore((s) => s.setSelectedConstraint);
  const exitSketchMode = useSketchStore((s) => s.exitSketchMode);
  const clearOperation = useSketchStore((s) => s.clearOperation);
  const saveProfile = useSketchStore((s) => s.saveProfile);
  const clearForNextProfile = useSketchStore((s) => s.clearForNextProfile);
  const exitLoftMode = useSketchStore((s) => s.exitLoftMode);
  const enterLoftMode = useSketchStore((s) => s.enterLoftMode);

  const addExtrude = useDocumentStore((s) => s.addExtrude);
  const addRevolve = useDocumentStore((s) => s.addRevolve);
  const addSweep = useDocumentStore((s) => s.addSweep);
  const addLoft = useDocumentStore((s) => s.addLoft);
  const select = useUiStore((s) => s.select);
  const setPreviewMesh = useEngineStore((s) => s.setPreviewMesh);
  const addToast = useNotificationStore((s) => s.addToast);

  const hasSegments = segments.length > 0;

  // Loft mode is a separate state machine — entering "loft" pending op should
  // also flip the loftMode flag so saveProfile/clearForNextProfile work.
  useEffect(() => {
    if (pendingOperation?.kind === "loft" && !loftMode) {
      enterLoftMode(plane);
    }
  }, [pendingOperation, loftMode, enterLoftMode, plane]);

  const selectedSegment =
    selectedSegments.length === 1 && constraintTool === "none"
      ? segments[selectedSegments[0]!]
      : null;
  const selectedConstraint =
    selectedConstraintIndex !== null ? constraints[selectedConstraintIndex] : null;

  function handleSaveProfile() {
    saveProfile();
    // Default next-profile origin: shift +10mm along plane normal.
    const { normal } = getSketchPlaneDirections(plane);
    const newOrigin = {
      x: origin.x + normal.x * 10,
      y: origin.y + normal.y * 10,
      z: origin.z + normal.z * 10,
    };
    clearForNextProfile(newOrigin);
  }

  // Exposed for SketchConfirmationCorner via a custom event so the corner
  // doesn't need to know all the addExtrude/addRevolve plumbing. Each caller
  // just dispatches `vcad:sketch-commit` and we handle the rest.
  useEffect(() => {
    if (!active) return;
    function handleCommit() {
      if (!pendingOperation) {
        // No pending op — just exit. Any drawn segments are dropped, so this
        // is an abandon from the funnel's perspective, not a completion.
        const status = exitSketchMode();
        analytics.sketchAbandoned(
          status === "has_segments" ? "no_operation" : "empty",
        );
        return;
      }
      if (!hasSegments && pendingOperation.kind !== "loft") {
        addToast(`Nothing to ${pendingOperation.kind} — draw a profile first`, "error");
        return;
      }

      try {
        if (pendingOperation.kind === "extrude") {
          const { normal } = getSketchPlaneDirections(plane);
          const depth = pendingOperation.flip ? -pendingOperation.depth : pendingOperation.depth;
          const direction = {
            x: normal.x * depth,
            y: normal.y * depth,
            z: normal.z * depth,
          };
          const twistRad = (pendingOperation.twistDeg * Math.PI) / 180;
          const partId = addExtrude(plane, origin, segments, direction, {
            twist_angle: Math.abs(pendingOperation.twistDeg) > 0.01 ? twistRad : undefined,
            scale_end:
              Math.abs(pendingOperation.scaleEnd - 1.0) > 0.01 ? pendingOperation.scaleEnd : undefined,
          });
          if (partId) {
            select(partId);
            analytics.extrudeApplied();
            analytics.sketchCompleted(constraints.length);
            addToast("Created Extrude", "success");
          } else {
            addToast("Extrude failed — sketch must form a closed loop", "error");
            return;
          }
        } else if (pendingOperation.kind === "revolve") {
          const { x_dir } = getSketchPlaneDirections(plane);
          const axisDir = pendingOperation.flip
            ? { x: -x_dir.x, y: -x_dir.y, z: -x_dir.z }
            : x_dir;
          const partId = addRevolve(plane, origin, segments, origin, axisDir, pendingOperation.angleDeg);
          if (partId) {
            select(partId);
            analytics.sketchCompleted(constraints.length);
            addToast("Created Revolve", "success");
          } else {
            addToast("Revolve failed", "error");
            return;
          }
        } else if (pendingOperation.kind === "sweep") {
          const { normal } = getSketchPlaneDirections(plane);
          const path =
            pendingOperation.pathType === "line"
              ? {
                  type: "Line" as const,
                  start: origin,
                  end: {
                    x: origin.x + normal.x * pendingOperation.height,
                    y: origin.y + normal.y * pendingOperation.height,
                    z: origin.z + normal.z * pendingOperation.height,
                  },
                }
              : {
                  type: "Helix" as const,
                  radius: pendingOperation.radius,
                  pitch: pendingOperation.height / pendingOperation.turns,
                  height: pendingOperation.height,
                  turns: pendingOperation.turns,
                };
          const partId = addSweep(plane, origin, segments, path);
          if (partId) {
            select(partId);
            analytics.sketchCompleted(constraints.length);
            addToast("Created Sweep", "success");
          } else {
            addToast("Sweep failed", "error");
            return;
          }
        } else if (pendingOperation.kind === "loft") {
          const allProfiles = exitLoftMode();
          if (allProfiles && allProfiles.length >= 2) {
            const partId = addLoft(
              allProfiles.map((p) => ({
                plane: p.plane,
                origin: p.origin,
                segments: p.segments,
              })),
              { closed: pendingOperation.closed },
            );
            if (partId) {
              select(partId);
              analytics.sketchCompleted(constraints.length);
              addToast("Created Loft", "success");
            }
          } else {
            addToast("Loft needs at least 2 profiles", "error");
            return;
          }
        }
      } catch (err) {
        console.error(`[sketch] ${pendingOperation.kind} commit failed:`, err);
        addToast(`${pendingOperation.kind} failed: ${String(err)}`, "error");
        return;
      }

      setPreviewMesh(null);
      clearOperation();
      exitSketchMode();
    }

    window.addEventListener("vcad:sketch-commit", handleCommit);
    return () => window.removeEventListener("vcad:sketch-commit", handleCommit);
  }, [
    active,
    pendingOperation,
    hasSegments,
    plane,
    origin,
    segments,
    constraints.length,
    addExtrude,
    addRevolve,
    addSweep,
    addLoft,
    exitLoftMode,
    select,
    addToast,
    setPreviewMesh,
    clearOperation,
    exitSketchMode,
  ]);

  if (!active) return null;

  return (
    <div className="space-y-1.5 text-sm px-2 pt-1.5 pb-2">
      {pendingOperation && (
        <OperationSection
          op={pendingOperation}
          hasSegments={hasSegments}
          profileCount={profiles.length}
          onSaveProfile={handleSaveProfile}
        />
      )}

      {selectedConstraint ? (
        <ConstraintSection
          constraint={selectedConstraint}
          index={selectedConstraintIndex!}
          onRemove={(i) => {
            removeConstraint(i);
            setSelectedConstraint(null);
          }}
        />
      ) : selectedSegment ? (
        <EntitySection
          segment={selectedSegment}
          index={selectedSegments[0]!}
          onClear={clearSelection}
        />
      ) : (
        <ToolSection
          tool={tool}
          constraintTool={constraintTool}
          selectedSegments={selectedSegments}
        />
      )}
    </div>
  );
}
