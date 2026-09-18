/**
 * The panel's one number input, so thirty of them do not each grow their own
 * rounding and their own empty-string behaviour.
 *
 * `parseFloat("")` is `NaN`, and a `NaN` that reaches the job request comes
 * back as a refusal naming a field the operator did not knowingly clear. So a
 * value that does not parse is simply not committed, and the box keeps what
 * was typed until it does.
 */
import { useEffect, useState } from "react";

interface NumberFieldProps {
  label: string;
  value: number;
  onCommit: (value: number) => void;
  step?: number;
  min?: number;
  /** Shown after the box, e.g. `mm` or `mm/min`. */
  unit?: string;
  disabled?: boolean;
  title?: string;
}

export function NumberField({
  label,
  value,
  onCommit,
  step = 0.1,
  min,
  unit,
  disabled,
  title,
}: NumberFieldProps) {
  const [text, setText] = useState(String(value));

  // Follow the store when something else moves the value — recommended feeds
  // land in five fields at once and the boxes have to show it.
  useEffect(() => {
    setText(String(value));
  }, [value]);

  return (
    <label className="block" title={title}>
      <span className="text-xs text-text-muted">
        {label}
        {unit ? ` (${unit})` : ""}
      </span>
      <input
        type="number"
        step={step}
        min={min}
        disabled={disabled}
        className="w-full bg-surface border border-border rounded px-2 py-1 text-sm disabled:opacity-50"
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          const parsed = parseFloat(e.target.value);
          if (Number.isFinite(parsed)) onCommit(parsed);
        }}
        onBlur={() => setText(String(value))}
      />
    </label>
  );
}
