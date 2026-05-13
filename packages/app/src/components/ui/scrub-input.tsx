import { useState, useEffect, useRef, useCallback } from "react";
import { Minus } from "@phosphor-icons/react/dist/ssr/Minus";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { cn } from "@/lib/utils";
import { Tooltip } from "./tooltip";

/**
 * True when the primary pointer is coarse (touch). Drives the mobile stepper
 * UI below. Checked at module load — pointer capability doesn't change mid-
 * session in practice.
 */
const isCoarsePointer =
  typeof window !== "undefined" &&
  typeof window.matchMedia === "function" &&
  window.matchMedia("(pointer: coarse)").matches;

function round(n: number, decimals = 3): number {
  const factor = Math.pow(10, decimals);
  return Math.round(n * factor) / factor;
}

interface ScrubInputProps {
  label: string;
  value: number;
  onChange: (value: number) => void;
  step?: number;
  min?: number;
  max?: number;
  unit?: string;
  className?: string;
  /** Compact mode for inline tree display (smaller, no unit text) */
  compact?: boolean;
  /** Tooltip text shown on hover over the label */
  tooltip?: string;
  /** Called when scrub drag begins */
  onScrubStart?: () => void;
  /** Called when scrub drag ends */
  onScrubEnd?: () => void;
  /**
   * Selection-primed entry key. When set, this input listens for
   * `vcad:prime-property-input` events and enters edit mode when the
   * event's `param` matches this value (e.g. "R", "H", "W", "D").
   */
  primeKey?: string;
  /** Called after the user commits a value (Enter/blur) while editing. */
  onCommit?: () => void;
}

export function ScrubInput({
  label,
  value,
  onChange,
  step = 1,
  min = -Infinity,
  max = Infinity,
  unit,
  className,
  compact = false,
  tooltip,
  onScrubStart,
  onScrubEnd,
  primeKey,
  onCommit,
}: ScrubInputProps) {
  const [text, setText] = useState(String(round(value)));
  const [isEditing, setIsEditing] = useState(false);
  const [isScrubbing, setIsScrubbing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const scrubStartX = useRef(0);
  const scrubStartValue = useRef(0);

  // Sync text with value when not editing
  useEffect(() => {
    if (!isEditing && !isScrubbing) {
      setText(String(round(value)));
    }
  }, [value, isEditing, isScrubbing]);

  function commit() {
    const num = parseFloat(text);
    if (!isNaN(num)) {
      const clamped = Math.max(min, Math.min(max, num));
      onChange(clamped);
    } else {
      setText(String(round(value)));
    }
    setIsEditing(false);
    onCommit?.();
  }

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Only scrub with mouse (coarse/touch devices get stepper buttons instead)
      if (isCoarsePointer || e.pointerType !== "mouse" || e.button !== 0 || isEditing) return;

      e.preventDefault();
      setIsScrubbing(true);
      scrubStartX.current = e.clientX;
      scrubStartValue.current = value;
      onScrubStart?.();
    },
    [isEditing, value, onScrubStart],
  );

  const bump = useCallback(
    (direction: 1 | -1) => {
      const newValue = round(value + direction * step);
      onChange(Math.max(min, Math.min(max, newValue)));
    },
    [value, step, min, max, onChange],
  );

  const handlePointerMove = useCallback(
    (e: PointerEvent) => {
      if (!isScrubbing) return;

      const deltaX = e.clientX - scrubStartX.current;
      scrubStartX.current = e.clientX;

      // Determine modifier
      let modifier = 1;
      if (e.shiftKey) modifier = 0.1; // fine
      if (e.altKey) modifier = 10; // coarse

      // Apply delta
      const delta = deltaX * step * modifier;
      const newValue = round(scrubStartValue.current + delta);
      const clamped = Math.max(min, Math.min(max, newValue));
      scrubStartValue.current = clamped;
      onChange(clamped);
    },
    [isScrubbing, step, min, max, onChange],
  );

  const handlePointerUp = useCallback(() => {
    if (!isScrubbing) return;
    setIsScrubbing(false);
    onScrubEnd?.();
  }, [isScrubbing, onScrubEnd]);

  // Global event listeners for scrubbing
  useEffect(() => {
    if (isScrubbing) {
      window.addEventListener("pointermove", handlePointerMove);
      window.addEventListener("pointerup", handlePointerUp);
      return () => {
        window.removeEventListener("pointermove", handlePointerMove);
        window.removeEventListener("pointerup", handlePointerUp);
      };
    }
  }, [isScrubbing, handlePointerMove, handlePointerUp]);

  // Selection-primed entry: listen for vcad:prime-property-input events
  useEffect(() => {
    if (!primeKey) return;
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<{ param: string }>).detail;
      if (detail?.param === primeKey) {
        setIsEditing(true);
        setTimeout(() => {
          inputRef.current?.select();
        }, 0);
      }
    };
    window.addEventListener("vcad:prime-property-input", handler);
    return () => window.removeEventListener("vcad:prime-property-input", handler);
  }, [primeKey]);

  function handleDoubleClick() {
    setIsEditing(true);
    setTimeout(() => inputRef.current?.select(), 0);
  }

  const labelSpan = (
    <span className={cn(
      "shrink-0 text-text-muted font-medium",
      compact ? "text-[9px] w-3" : "text-[10px] w-4"
    )}>{label}</span>
  );

  return (
    <label className={cn("flex items-center gap-1.5 text-xs", className)}>
      {tooltip ? <Tooltip content={tooltip} side="top">{labelSpan}</Tooltip> : labelSpan}
      {isCoarsePointer && !compact && (
        <button
          type="button"
          onClick={() => bump(-1)}
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border bg-card text-text-muted active:bg-hover"
          aria-label={`Decrease ${label}`}
        >
          <Minus size={14} />
        </button>
      )}
      <input
        ref={inputRef}
        type={isCoarsePointer && isEditing ? "number" : "text"}
        inputMode={isCoarsePointer ? "decimal" : undefined}
        value={isEditing ? text : String(round(value))}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onFocus={() => {
          if (isCoarsePointer) {
            setIsEditing(true);
            setTimeout(() => inputRef.current?.select(), 0);
          }
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") {
            setText(String(round(value)));
            setIsEditing(false);
          }
        }}
        onPointerDown={handlePointerDown}
        onDoubleClick={handleDoubleClick}
        readOnly={!isEditing && !isCoarsePointer}
        className={cn(
          "flex-1 min-w-0 rounded-md bg-card border border-border text-text outline-none transition-colors text-center",
          "hover:border-text-muted focus:border-brand",
          !isEditing && !isCoarsePointer && "cursor-ew-resize select-none",
          isScrubbing && "cursor-ew-resize",
          compact ? "px-1 py-0.5 text-[10px]" : "px-2 py-1 text-xs",
          isCoarsePointer && !compact && "h-9 text-sm tabular-nums",
        )}
      />
      {isCoarsePointer && !compact && (
        <button
          type="button"
          onClick={() => bump(1)}
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border bg-card text-text-muted active:bg-hover"
          aria-label={`Increase ${label}`}
        >
          <Plus size={14} />
        </button>
      )}
      {unit && !compact && <span className="text-[10px] text-text-muted shrink-0">{unit}</span>}
    </label>
  );
}
