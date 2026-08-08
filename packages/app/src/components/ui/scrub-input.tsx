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
  /**
   * The expression currently driving this field, if it is bound to one.
   * When set, the input shows the formula rather than the number, and the
   * resolved value moves into the unit slot.
   */
  expression?: string | null;
  /**
   * Accept expressions, not just numbers. Called when the user types
   * something that is not a number but *is* a valid expression over the
   * document's parameters — `wall`, `bore * 0.5`, `plate_t + 2`.
   *
   * Without this, a typed formula is silently discarded and the field snaps
   * back to its number: the failure mode that made parameters unreachable
   * from the property panel. Pass `null` back to clear an existing binding.
   */
  onBind?: (expression: string | null) => void;
  /**
   * Validate an expression before binding. Return an error string to reject
   * it (unknown parameter, parse error) — the input shows the message and
   * keeps the text so the user can fix it rather than losing what they typed.
   */
  validateExpression?: (expression: string) => string | null;
  /**
   * What the bound expression currently evaluates to. Shown instead of
   * `value`, which for a bound field is the document's *unbound* literal —
   * bindings resolve at evaluation time, so `value` never moves.
   */
  resolvedValue?: number;
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
  expression = null,
  onBind,
  validateExpression,
  resolvedValue,
}: ScrubInputProps) {
  const display = expression ?? String(round(value));
  const [text, setText] = useState(display);
  const [isEditing, setIsEditing] = useState(false);
  const [isScrubbing, setIsScrubbing] = useState(false);
  const [bindError, setBindError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const scrubStartX = useRef(0);
  const scrubStartValue = useRef(0);

  // Sync text with value (or the bound expression) when not editing
  useEffect(() => {
    if (!isEditing && !isScrubbing) {
      setText(expression ?? String(round(value)));
      setBindError(null);
    }
  }, [value, expression, isEditing, isScrubbing]);

  function commit() {
    const trimmed = text.trim();
    const num = parseFloat(trimmed);
    // A bare number always wins, and clears any binding that was there —
    // typing a literal over a formula is how you unbind.
    if (!isNaN(num) && String(num) === trimmed) {
      const clamped = Math.max(min, Math.min(max, num));
      if (expression && onBind) onBind(null);
      onChange(clamped);
      setBindError(null);
    } else if (trimmed.length > 0 && onBind) {
      const err = validateExpression?.(trimmed) ?? null;
      if (err) {
        // Keep what the user typed — losing a half-written formula on a
        // typo is worse than showing the typo.
        setBindError(err);
        setIsEditing(false);
        onCommit?.();
        return;
      }
      onBind(trimmed);
      setBindError(null);
    } else if (!isNaN(num)) {
      // Numeric with trailing junk ("12mm") — take the number.
      onChange(Math.max(min, Math.min(max, num)));
      setBindError(null);
    } else {
      setText(expression ?? String(round(value)));
      setBindError(null);
    }
    setIsEditing(false);
    onCommit?.();
  }

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Only scrub with mouse (coarse/touch devices get stepper buttons instead)
      if (isCoarsePointer || e.pointerType !== "mouse" || e.button !== 0 || isEditing) return;
      // Scrubbing a bound field would write a number over the formula.
      if (expression != null) return;

      e.preventDefault();
      setIsScrubbing(true);
      scrubStartX.current = e.clientX;
      scrubStartValue.current = value;
      onScrubStart?.();
    },
    [isEditing, value, onScrubStart, expression],
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

  const isBound = expression != null;

  return (
    <label
      className={cn("flex items-center gap-1.5 text-xs", className)}
      title={
        bindError ??
        (isBound
          ? `${expression} = ${round(resolvedValue ?? value)}${unit ? ` ${unit}` : ""}`
          : undefined)
      }
    >
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
        value={isEditing ? text : display}
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
            setText(display);
            setIsEditing(false);
            setBindError(null);
          }
        }}
        onPointerDown={handlePointerDown}
        onDoubleClick={handleDoubleClick}
        readOnly={!isEditing && !isCoarsePointer}
        className={cn(
          "flex-1 min-w-0 rounded-md bg-card border text-text outline-none transition-colors text-center",
          "hover:border-text-muted focus:border-brand",
          bindError
            ? "border-red-500"
            : isBound
              ? "border-brand/50 text-brand font-mono"
              : "border-border",
          // A bound field's value comes from its formula — dragging it would
          // be a lie. Double-click still edits (to rebind or unbind).
          !isEditing && !isCoarsePointer && !isBound && "cursor-ew-resize select-none",
          !isEditing && isBound && "cursor-text",
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
      {!compact && (
        <span
          className={cn(
            "text-[10px] shrink-0",
            bindError ? "text-red-500" : "text-text-muted",
          )}
        >
          {/* A bound field spends its own slot on the formula, so the
              resolved number moves here — you can still see what it is. */}
          {isBound ? `= ${round(resolvedValue ?? value)}` : unit}
        </span>
      )}
    </label>
  );
}
