import { useEffect, useMemo, useRef, useState } from "react";
import { Link } from "@phosphor-icons/react/dist/ssr/Link";
import { LinkBreak } from "@phosphor-icons/react/dist/ssr/LinkBreak";
import {
  evalExprSafe,
  ExpressionParseError,
  evaluateExpression,
  parseExpression,
} from "@vcad/engine";
import { useParametersStore } from "@vcad/core";
import { cn } from "@/lib/utils";
import { ScrubInput } from "./scrub-input";
import { Tooltip } from "./tooltip";

interface ExpressionInputProps {
  /** Target node id (used to key the binding, e.g. `1:size.x`). */
  nodeId: string | number;
  /** Dotted field path (e.g. "size.x", "radius", "offset.z"). */
  fieldPath: string;
  label: string;
  /** Current numeric value (what the field currently holds). */
  value: number;
  /** Called with a number when the user commits a literal value. */
  onChange: (value: number) => void;
  step?: number;
  min?: number;
  max?: number;
  unit?: string;
  className?: string;
  compact?: boolean;
  tooltip?: string;
  onScrubStart?: () => void;
  onScrubEnd?: () => void;
}

/**
 * Scrub-input that transparently supports expression bindings.
 *
 * When the field is bound to a formula (looked up via the parameters store
 * using `{nodeId}:{fieldPath}`), the widget renders as a read-only pill
 * showing the formula plus its evaluated preview. Double-click (or pressing
 * `=`) switches back to text editing; typing a pure number and pressing
 * Enter unbinds and commits a literal; typing a formula (re-)binds.
 */
export function ExpressionInput({
  nodeId,
  fieldPath,
  label,
  value,
  onChange,
  step,
  min,
  max,
  unit,
  className,
  compact,
  tooltip,
  onScrubStart,
  onScrubEnd,
}: ExpressionInputProps) {
  const binding = useParametersStore(
    (s) => s.bindings[`${nodeId}:${fieldPath}`],
  );
  const setBinding = useParametersStore((s) => s.setBinding);
  const removeBinding = useParametersStore((s) => s.removeBinding);
  const parameters = useParametersStore((s) => s.parameters);

  // Build a plain {name: number} env lazily; cheap for reasonable param counts.
  const env = useMemo(() => {
    const out: Record<string, number> = {};
    for (const [name, p] of Object.entries(parameters)) {
      try {
        out[name] = typeof p.value === "number" ? p.value : evaluateExpression(p.value, out);
      } catch {
        out[name] = Number.NaN;
      }
    }
    return out;
  }, [parameters]);

  const [isEditing, setIsEditing] = useState(false);
  const [draft, setDraft] = useState<string>(
    binding != null ? String(binding) : "",
  );
  const [parseErr, setParseErr] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Sync draft if binding changes externally.
  useEffect(() => {
    if (!isEditing) {
      setDraft(binding != null ? String(binding) : "");
      setParseErr(null);
    }
  }, [binding, isEditing]);

  const boundFormula = typeof binding === "string" ? binding : null;
  const preview = boundFormula ? evalExprSafe(boundFormula, env) : value;

  function commit(text: string) {
    const trimmed = text.trim();
    setParseErr(null);
    if (!trimmed) {
      removeBinding(nodeId, fieldPath);
      setIsEditing(false);
      return;
    }
    // Bare number: unbind and commit as literal.
    const n = Number(trimmed);
    if (!Number.isNaN(n) && Number.isFinite(n)) {
      removeBinding(nodeId, fieldPath);
      onChange(n);
      setIsEditing(false);
      return;
    }
    // Expression: validate and bind.
    try {
      parseExpression(trimmed);
      setBinding(nodeId, fieldPath, trimmed);
      setIsEditing(false);
    } catch (e) {
      setParseErr(e instanceof ExpressionParseError ? e.message : "invalid expression");
    }
  }

  // Not bound and not editing → fall through to scrub input.
  if (boundFormula == null && !isEditing) {
    return (
      <ScrubInput
        label={label}
        value={value}
        onChange={(n) => {
          // Guard: if the user typed an expression into the scrub input
          // instead of a number, promote to a binding.
          // ScrubInput already filters non-numeric text via parseFloat,
          // so we only hit this path for numeric commits.
          onChange(n);
        }}
        step={step}
        min={min}
        max={max}
        unit={unit}
        className={className}
        compact={compact}
        tooltip={tooltip}
        onScrubStart={onScrubStart}
        onScrubEnd={onScrubEnd}
      />
    );
  }

  // Bound or editing: render a text-pill UI.
  const previewText = Number.isFinite(preview)
    ? `= ${round(preview, 3)}${unit ? ` ${unit}` : ""}`
    : "= ?";
  const isBound = boundFormula != null;

  const labelSpan = (
    <span
      className={cn(
        "shrink-0 text-text-muted font-medium",
        compact ? "text-[9px] w-3" : "text-[10px] w-4",
      )}
    >
      {label}
    </span>
  );

  return (
    <label className={cn("flex items-center gap-1.5 text-xs", className)}>
      {tooltip ? (
        <Tooltip content={tooltip} side="top">
          {labelSpan}
        </Tooltip>
      ) : (
        labelSpan
      )}
      <span
        className={cn(
          "flex items-center justify-center",
          compact ? "h-4 w-3" : "h-5 w-4",
          isBound ? "text-brand" : "text-text-muted",
        )}
        aria-hidden
      >
        {isBound ? <Link size={10} /> : <LinkBreak size={10} />}
      </span>
      <input
        ref={inputRef}
        type="text"
        value={isEditing ? draft : (boundFormula ?? "")}
        placeholder={isBound ? undefined : "expression"}
        onChange={(e) => {
          setIsEditing(true);
          setDraft(e.target.value);
          setParseErr(null);
        }}
        onFocus={() => setIsEditing(true)}
        onBlur={() => commit(draft)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.currentTarget.blur();
          } else if (e.key === "Escape") {
            setDraft(boundFormula ?? "");
            setIsEditing(false);
            setParseErr(null);
          }
        }}
        title={parseErr ?? `${boundFormula ?? "(unbound)"} ${previewText}`}
        className={cn(
          "flex-1 min-w-0 bg-card border text-text outline-none transition-colors font-mono",
          parseErr
            ? "border-red-500"
            : isBound
              ? "border-brand focus:border-brand"
              : "border-border focus:border-brand",
          compact ? "px-1 py-0.5 text-[10px]" : "px-2 py-1 text-xs",
        )}
      />
      <span
        className={cn(
          "text-[10px] text-text-muted shrink-0 tabular-nums",
          parseErr && "text-red-500",
        )}
        style={{ minWidth: compact ? 32 : 48 }}
      >
        {parseErr ? "!" : previewText}
      </span>
    </label>
  );
}

function round(n: number, decimals = 3): number {
  const f = Math.pow(10, decimals);
  return Math.round(n * f) / f;
}
