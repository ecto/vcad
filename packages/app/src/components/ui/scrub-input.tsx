import { useState, useEffect, useRef, useCallback } from "react";
import { cn } from "@/lib/utils";
import { Tooltip } from "./tooltip";

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
  }

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Only scrub with left mouse button and not when editing
      if (e.button !== 0 || isEditing) return;

      e.preventDefault();
      setIsScrubbing(true);
      scrubStartX.current = e.clientX;
      scrubStartValue.current = value;
      onScrubStart?.();
    },
    [isEditing, value, onScrubStart],
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
      <input
        ref={inputRef}
        type="text"
        value={isEditing ? text : String(round(value))}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") {
            setText(String(round(value)));
            setIsEditing(false);
          }
        }}
        onPointerDown={handlePointerDown}
        onDoubleClick={handleDoubleClick}
        readOnly={!isEditing}
        className={cn(
          "flex-1 min-w-0 bg-card border border-border text-xs text-text outline-none transition-colors",
          "hover:border-text-muted focus:border-accent",
          !isEditing && "cursor-ew-resize select-none",
          isScrubbing && "cursor-ew-resize",
          compact ? "px-1 py-0.5 text-[10px]" : "px-2 py-1",
        )}
      />
      {unit && !compact && <span className="text-[10px] text-text-muted shrink-0">{unit}</span>}
    </label>
  );
}
