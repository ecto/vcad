import { useCallback, useEffect, useRef, useState } from "react";
import {
  formatLength,
  toMm,
  useUiStore,
  UNIT_LABEL,
  type LengthUnit,
} from "@vcad/core";
import { FooterChip } from "@/components/footer/FooterChip";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

type Axis = "x" | "y" | "z";

const AXES: Axis[] = ["x", "y", "z"];

/**
 * Cursor world coordinates chip.
 *
 * Shows live mouse-on-ground-plane XYZ in the user's chosen length unit
 * (Z-up, kernel space). Each axis is click-to-edit: focus opens an inline
 * input populated with the current value; Enter dispatches a `vcad:focus-point`
 * event that pans the camera to the typed point. Tab moves between axes.
 * Escape cancels.
 *
 * The unit suffix is itself a click target — cycles mm → cm → in.
 */
export function CursorCoordChip({ className }: { className?: string }) {
  const cursorWorld = useUiStore((s) => s.cursorWorld);
  const lengthUnit = useUiStore((s) => s.lengthUnit);
  const cycleLengthUnit = useUiStore((s) => s.cycleLengthUnit);

  const [editing, setEditing] = useState<Axis | null>(null);
  const [drafts, setDrafts] = useState<Record<Axis, string>>({ x: "", y: "", z: "" });
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Snapshot the cursor at edit-start so the user types against a fixed point
  // rather than the wiggling live raycast.
  const editAnchorRef = useRef<{ x: number; y: number; z: number } | null>(null);

  const startEdit = useCallback(
    (axis: Axis) => {
      const anchor = cursorWorld ?? { x: 0, y: 0, z: 0 };
      editAnchorRef.current = anchor;
      setDrafts({
        x: formatLength(anchor.x, lengthUnit),
        y: formatLength(anchor.y, lengthUnit),
        z: formatLength(anchor.z, lengthUnit),
      });
      setEditing(axis);
    },
    [cursorWorld, lengthUnit],
  );

  const commit = useCallback(
    (focus: boolean) => {
      const anchor = editAnchorRef.current ?? { x: 0, y: 0, z: 0 };
      const parsed: Record<Axis, number> = {
        x: parseFloat(drafts.x),
        y: parseFloat(drafts.y),
        z: parseFloat(drafts.z),
      };
      const target = {
        x: Number.isFinite(parsed.x) ? toMm(parsed.x, lengthUnit) : anchor.x,
        y: Number.isFinite(parsed.y) ? toMm(parsed.y, lengthUnit) : anchor.y,
        z: Number.isFinite(parsed.z) ? toMm(parsed.z, lengthUnit) : anchor.z,
      };
      setEditing(null);
      editAnchorRef.current = null;
      if (focus) {
        window.dispatchEvent(
          new CustomEvent("vcad:focus-point", { detail: target }),
        );
      }
    },
    [drafts, lengthUnit],
  );

  const cancel = useCallback(() => {
    setEditing(null);
    editAnchorRef.current = null;
  }, []);

  useEffect(() => {
    if (editing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editing]);

  // Per-axis visibility — Z drops first, then Y, then the whole chip is
  // hidden by `className` from the parent at sm breakpoint. X is always
  // visible while the chip is mounted.
  const AXIS_VISIBILITY: Record<Axis, string> = {
    x: "",
    y: "hidden md:inline-flex",
    z: "hidden xl:inline-flex",
  };

  return (
    <Tooltip
      side="top"
      content="Cursor on the ground plane (Z-up). Click an axis to type a value and pan there."
    >
    <FooterChip
      className={cn("hidden sm:flex tabular-nums gap-1.5", className)}
    >
      {AXES.map((axis) => (
        <AxisField
          key={axis}
          axis={axis}
          editing={editing === axis}
          inputRef={editing === axis ? inputRef : undefined}
          value={cursorWorld ? cursorWorld[axis] : null}
          unit={lengthUnit}
          draft={drafts[axis]}
          onDraft={(v) => setDrafts((d) => ({ ...d, [axis]: v }))}
          onStart={() => startEdit(axis)}
          onCommit={() => commit(true)}
          onCancel={cancel}
          onTab={(shift) => {
            const idx = AXES.indexOf(axis);
            const nextIdx = shift ? (idx + 2) % 3 : (idx + 1) % 3;
            setEditing(AXES[nextIdx] ?? axis);
          }}
          className={AXIS_VISIBILITY[axis]}
        />
      ))}
      <button
        type="button"
        onClick={cycleLengthUnit}
        className={cn(
          "uppercase tracking-wide text-text-muted/60",
          "hover:text-text px-1 -mx-1 rounded transition-colors",
        )}
      >
        {UNIT_LABEL[lengthUnit]}
      </button>
    </FooterChip>
    </Tooltip>
  );
}

interface AxisFieldProps {
  axis: Axis;
  editing: boolean;
  inputRef?: React.RefObject<HTMLInputElement | null>;
  value: number | null;
  unit: LengthUnit;
  draft: string;
  onDraft: (v: string) => void;
  onStart: () => void;
  onCommit: () => void;
  onCancel: () => void;
  onTab: (shift: boolean) => void;
  className?: string;
}

function AxisField({
  axis,
  editing,
  inputRef,
  value,
  unit,
  draft,
  onDraft,
  onStart,
  onCommit,
  onCancel,
  onTab,
  className,
}: AxisFieldProps) {
  const display = value !== null ? formatLength(value, unit) : null;
  return (
    <span className={cn("inline-flex items-center", className)}>
      <span className="text-brand">{axis}</span>
      {editing ? (
        <input
          ref={inputRef}
          type="text"
          inputMode="decimal"
          value={draft}
          onChange={(e) => onDraft(e.target.value)}
          onBlur={onCommit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onCommit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
            } else if (e.key === "Tab") {
              e.preventDefault();
              onTab(e.shiftKey);
            }
          }}
          className={cn(
            "w-12 bg-hover text-text outline-none",
            "px-1 ml-0.5 tabular-nums",
            "border border-brand/60 rounded-sm",
          )}
        />
      ) : (
        <button
          type="button"
          onClick={onStart}
          className={cn(
            "w-12 text-right tabular-nums",
            "hover:text-text rounded-sm transition-colors",
            display === null && "opacity-40",
          )}
        >
          {display ?? "  —"}
        </button>
      )}
    </span>
  );
}
