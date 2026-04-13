import { useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/utils";

const LEFT_KEY = "vcad:layout:left-width";
const RIGHT_KEY = "vcad:layout:right-width";

const DEFAULT_LEFT = 220;
const DEFAULT_RIGHT = 260;
const MIN_SIDEBAR = 32;   // collapsed icon-strip width
const MAX_SIDEBAR = 480;

function loadWidth(key: string, fallback: number): number {
  if (typeof localStorage === "undefined") return fallback;
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return fallback;
    const n = Number(raw);
    if (!Number.isFinite(n)) return fallback;
    return Math.min(MAX_SIDEBAR, Math.max(MIN_SIDEBAR, n));
  } catch {
    return fallback;
  }
}

function saveWidth(key: string, value: number): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(key, String(value));
  } catch {
    /* ignore quota errors */
  }
}

interface ResizeHandleProps {
  side: "left" | "right";
  width: number;
  onResize: (next: number) => void;
}

/**
 * Invisible 8px-wide drag hit zone overlaid on the sidebar's 1px border.
 * Visually it's just the border; functionally it's grabbable. Hover paints
 * an accent line on top of the border so users get feedback.
 */
function ResizeHandle({ side, width, onResize }: ResizeHandleProps) {
  const dragging = useRef(false);
  const startX = useRef(0);
  const startWidth = useRef(0);
  const [hover, setHover] = useState(false);

  useEffect(() => {
    function onMove(e: MouseEvent) {
      if (!dragging.current) return;
      const dx = e.clientX - startX.current;
      const delta = side === "left" ? dx : -dx;
      const next = Math.min(MAX_SIDEBAR, Math.max(MIN_SIDEBAR, startWidth.current + delta));
      onResize(next);
    }
    function onUp() {
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [side, onResize]);

  // The handle is absolutely positioned, straddling the column edge. Width is
  // 8px hit zone; the visible 1px border is owned by the sidebar column itself.
  return (
    <div
      onMouseDown={(e) => {
        dragging.current = true;
        startX.current = e.clientX;
        startWidth.current = width;
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
      }}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      className={cn(
        "absolute top-0 bottom-0 z-10 w-2 cursor-col-resize",
        side === "left" ? "-right-1" : "-left-1",
      )}
      aria-label={`Resize ${side} sidebar`}
      role="separator"
    >
      {(hover || dragging.current) && (
        <div
          className={cn(
            "absolute top-0 bottom-0 w-px bg-accent",
            side === "left" ? "right-1" : "left-1",
          )}
        />
      )}
    </div>
  );
}

interface AppShellProps {
  header?: ReactNode;
  leftSidebar?: ReactNode;
  rightSidebar?: ReactNode;
  /** Optional dock above the footer (e.g. DevTools / log panel) */
  bottomDock?: ReactNode;
  /** Bottom status bar (Borland F-key hint row) */
  footer?: ReactNode;
  /** Main viewport content — takes the remaining flex space in the body row. */
  children: ReactNode;
}

/**
 * IDE-style shell with named slots:
 *   Row 1: header       (menu bar + tool palette as one unified chrome)
 *   Row 2: body — [leftSidebar] [viewport] [rightSidebar]
 *   Row 3: footer       (Borland-style F-key hint row)
 *
 * Sidebar widths are drag-resizable and persist to localStorage.
 */
export function AppShell({
  header,
  leftSidebar,
  rightSidebar,
  bottomDock,
  footer,
  children,
}: AppShellProps) {
  const [leftWidth, setLeftWidth] = useState(() => loadWidth(LEFT_KEY, DEFAULT_LEFT));
  const [rightWidth, setRightWidth] = useState(() => loadWidth(RIGHT_KEY, DEFAULT_RIGHT));

  useEffect(() => { saveWidth(LEFT_KEY, leftWidth); }, [leftWidth]);
  useEffect(() => { saveWidth(RIGHT_KEY, rightWidth); }, [rightWidth]);

  return (
    <div className="flex h-[100dvh] w-screen flex-col overflow-hidden bg-bg">
      {header && (
        <div className="shrink-0 border-b border-border">
          {header}
        </div>
      )}
      <div className="flex flex-1 min-h-0 flex-row">
        {leftSidebar && (
          <div
            className="relative shrink-0 min-h-0 overflow-visible border-r border-border bg-surface"
            style={{ width: `${leftWidth}px` }}
          >
            <div className="h-full w-full overflow-hidden">{leftSidebar}</div>
            <ResizeHandle side="left" width={leftWidth} onResize={setLeftWidth} />
          </div>
        )}
        <div className="relative flex-1 min-w-0 min-h-0">
          {children}
        </div>
        {rightSidebar && (
          <div
            className="relative shrink-0 min-h-0 overflow-visible border-l border-border bg-surface"
            style={{ width: `${rightWidth}px` }}
          >
            <ResizeHandle side="right" width={rightWidth} onResize={setRightWidth} />
            <div className="h-full w-full overflow-hidden">{rightSidebar}</div>
          </div>
        )}
      </div>
      {bottomDock && (
        <div className="shrink-0 border-t border-border">
          {bottomDock}
        </div>
      )}
      {footer && (
        <div className="shrink-0 border-t border-border">
          {footer}
        </div>
      )}
    </div>
  );
}
