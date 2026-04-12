import { useEffect, useRef, useState, type ReactNode } from "react";

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
 * Narrow vertical drag handle for resizing a sidebar.
 * Lives between a sidebar column and the viewport.
 */
function ResizeHandle({ side, width, onResize }: ResizeHandleProps) {
  const dragging = useRef(false);
  const startX = useRef(0);
  const startWidth = useRef(0);

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

  return (
    <div
      onMouseDown={(e) => {
        dragging.current = true;
        startX.current = e.clientX;
        startWidth.current = width;
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
      }}
      className="w-[4px] shrink-0 cursor-col-resize bg-border/30 hover:bg-accent transition-colors"
      aria-label={`Resize ${side} sidebar`}
      role="separator"
    />
  );
}

interface AppShellProps {
  header?: ReactNode;
  palette?: ReactNode;
  leftSidebar?: ReactNode;
  rightSidebar?: ReactNode;
  /** Bottom status bar (Borland F-key hint row) */
  footer?: ReactNode;
  /** Main viewport content — takes the remaining flex space in the body row. */
  children: ReactNode;
}

/**
 * IDE-style shell with named slots:
 *   Row 1: header       (e.g. logo, doc name, user menu)
 *   Row 2: tool palette (e.g. Borland-style tabbed component palette)
 *   Row 3: body — [leftSidebar] [viewport] [rightSidebar]
 *
 * Sidebar widths are drag-resizable and persist to localStorage. Unknown
 * slots collapse to zero height/width so the viewport fills the available
 * space on routes that don't need them.
 */
export function AppShell({
  header,
  palette,
  leftSidebar,
  rightSidebar,
  footer,
  children,
}: AppShellProps) {
  const [leftWidth, setLeftWidth] = useState(() => loadWidth(LEFT_KEY, DEFAULT_LEFT));
  const [rightWidth, setRightWidth] = useState(() => loadWidth(RIGHT_KEY, DEFAULT_RIGHT));

  useEffect(() => { saveWidth(LEFT_KEY, leftWidth); }, [leftWidth]);
  useEffect(() => { saveWidth(RIGHT_KEY, rightWidth); }, [rightWidth]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-bg">
      {header && (
        <div className="shrink-0 border-b border-border">
          {header}
        </div>
      )}
      {palette && (
        <div className="shrink-0 border-b border-border">
          {palette}
        </div>
      )}
      <div className="flex flex-1 min-h-0 flex-row">
        {leftSidebar && (
          <>
            <div
              className="shrink-0 min-h-0 overflow-hidden border-r border-border bg-card"
              style={{ width: `${leftWidth}px` }}
            >
              {leftSidebar}
            </div>
            <ResizeHandle side="left" width={leftWidth} onResize={setLeftWidth} />
          </>
        )}
        <div className="relative flex-1 min-w-0 min-h-0">
          {children}
        </div>
        {rightSidebar && (
          <>
            <ResizeHandle side="right" width={rightWidth} onResize={setRightWidth} />
            <div
              className="shrink-0 min-h-0 overflow-hidden border-l border-border bg-card"
              style={{ width: `${rightWidth}px` }}
            >
              {rightSidebar}
            </div>
          </>
        )}
      </div>
      {footer && (
        <div className="shrink-0 border-t border-border">
          {footer}
        </div>
      )}
    </div>
  );
}
