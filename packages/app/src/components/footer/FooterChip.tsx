import {
  forwardRef,
  type ButtonHTMLAttributes,
  type HTMLAttributes,
  type ReactNode,
} from "react";
import { cn } from "@/lib/utils";

export type ChipSeverity = "muted" | "info" | "brand" | "warn" | "danger" | "success";

const SEVERITY_TEXT: Record<ChipSeverity, string> = {
  muted: "text-text-muted",
  info: "text-text",
  brand: "text-brand",
  warn: "text-amber-400",
  danger: "text-red-400",
  success: "text-emerald-400",
};

interface ChipBase {
  divider?: boolean;
  flex?: boolean;
  severity?: ChipSeverity;
}

function chipClass({
  divider = true,
  flex = false,
  severity = "muted",
  className,
  interactive,
}: ChipBase & { className?: string; interactive: boolean }): string {
  return cn(
    "flex items-center gap-2 px-3 whitespace-pre",
    "text-[10px] font-mono",
    SEVERITY_TEXT[severity],
    divider && "border-l border-border/40",
    flex && "flex-1 min-w-0",
    interactive &&
      "hover:bg-hover hover:text-text focus:outline-none focus-visible:bg-hover transition-colors",
    className,
  );
}

type FooterChipProps = ChipBase & HTMLAttributes<HTMLDivElement>;

/**
 * Inert chip slot for the StatusBar footer.
 *
 * Shapes density and styling (divider, padding, severity color, font) so each
 * footer section reads as one cell of the same grid. Render arbitrary content
 * inside; the chip just controls the frame.
 *
 * - `divider`: left border separator. Default true; pass `false` for the
 *   leading chip in a row.
 * - `flex`: chip expands to fill available width (use for the ticker).
 * - `severity`: text color cue.
 *
 * Uses `forwardRef` so this composes with Radix triggers via `asChild`.
 */
export const FooterChip = forwardRef<HTMLDivElement, FooterChipProps>(
  function FooterChip({ divider, flex, severity, className, children, ...rest }, ref) {
    return (
      <div
        ref={ref}
        className={chipClass({ divider, flex, severity, className, interactive: false })}
        {...rest}
      >
        {children}
      </div>
    );
  },
);

type FooterChipButtonProps = ChipBase & ButtonHTMLAttributes<HTMLButtonElement>;

/**
 * Interactive variant of FooterChip — same frame, plus hover/focus feedback
 * and `cursor-pointer`. Renders a `<button>` so it works as a Radix
 * Popover/HoverCard trigger via `asChild`.
 */
export const FooterChipButton = forwardRef<HTMLButtonElement, FooterChipButtonProps>(
  function FooterChipButton(
    { divider, flex, severity, className, type = "button", children, ...rest },
    ref,
  ) {
    return (
      <button
        ref={ref}
        type={type}
        className={chipClass({ divider, flex, severity, className, interactive: true })}
        {...rest}
      >
        {children}
      </button>
    );
  },
);

interface ChipLabelProps {
  children: ReactNode;
  className?: string;
}

/** Faint uppercase label for a chip's prefix (e.g., "PARTS", "FPS"). */
export function ChipLabel({ children, className }: ChipLabelProps) {
  return (
    <span className={cn("uppercase tracking-wide text-text-muted/70", className)}>
      {children}
    </span>
  );
}

interface ChipValueProps {
  children: ReactNode;
  className?: string;
}

/** Tabular-nums numeric value, sized to align across chips. */
export function ChipValue({ children, className }: ChipValueProps) {
  return <span className={cn("tabular-nums", className)}>{children}</span>;
}
