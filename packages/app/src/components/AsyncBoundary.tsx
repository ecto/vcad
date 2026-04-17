import {
  Component,
  Suspense,
  type ErrorInfo,
  type ReactNode,
} from "react";
import { analytics } from "@/lib/analytics";

interface AsyncBoundaryProps {
  /**
   * Stable region name used for telemetry and to group related lazy
   * components (e.g. "property-panel", "chat-sidebar"). Don't interpolate
   * per-render state — analytics groups by this value.
   */
  region: string;
  /** Rendered while a child's lazy chunk is loading. */
  fallback?: ReactNode;
  /**
   * Optional custom error UI. Receives the error, a classification, and
   * a `recover` callback. If omitted, a default inline fallback is shown
   * sized to fit typical side-panel / overlay slots.
   */
  renderError?: (args: {
    error: Error;
    kind: "chunk" | "runtime";
    recover: () => void;
  }) => ReactNode;
  children: ReactNode;
}

interface AsyncBoundaryState {
  error: Error | null;
  /** Bumped on user-initiated retry to force a subtree remount. */
  resetKey: number;
}

function classifyError(err: Error): "chunk" | "runtime" {
  const msg = err.message || "";
  // Match the transient/chunk patterns from lazy-with-retry plus a couple
  // of variants browsers emit for module script failures.
  return /Failed to fetch|NetworkError|network connection|Importing a module script failed|Load failed|error loading dynamically imported module|ChunkLoadError/i.test(
    msg,
  )
    ? "chunk"
    : "runtime";
}

/**
 * Per-region Suspense + error boundary for lazy-loaded subtrees.
 *
 * Pairs with `lazyWithRetry`: the retry helper handles transient network
 * hiccups silently; this boundary catches what survives that (runtime errors
 * inside the loaded component, or a chunk that genuinely can't be fetched).
 *
 * Recovery strategy differs by error kind:
 *   - `chunk`   → offer a full reload. React.lazy memoizes rejected promises,
 *                 so a key-bump remount cannot re-trigger the import within
 *                 the same page session.
 *   - `runtime` → bump `resetKey` to remount the subtree. The lazy module
 *                 is already resolved, so remount is cheap and in-place.
 */
export class AsyncBoundary extends Component<
  AsyncBoundaryProps,
  AsyncBoundaryState
> {
  state: AsyncBoundaryState = { error: null, resetKey: 0 };

  static getDerivedStateFromError(error: Error): Partial<AsyncBoundaryState> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(
      `[AsyncBoundary:${this.props.region}]`,
      error,
      info.componentStack,
    );
    analytics.asyncBoundaryCaught(this.props.region, error.message);
  }

  private recover = () => {
    const { error } = this.state;
    if (!error) return;
    if (classifyError(error) === "chunk") {
      window.location.reload();
      return;
    }
    analytics.asyncBoundaryReset(this.props.region);
    this.setState((s) => ({ error: null, resetKey: s.resetKey + 1 }));
  };

  render() {
    const { error, resetKey } = this.state;
    const { fallback = null, renderError, children } = this.props;

    if (error) {
      const kind = classifyError(error);
      if (renderError) {
        return <>{renderError({ error, kind, recover: this.recover })}</>;
      }
      return <DefaultErrorFallback kind={kind} onRecover={this.recover} />;
    }

    return (
      <Suspense key={resetKey} fallback={fallback}>
        {children}
      </Suspense>
    );
  }

  // Use the region name for debug tools (React DevTools displayName).
  static displayName = "AsyncBoundary";
}

function DefaultErrorFallback({
  kind,
  onRecover,
}: {
  kind: "chunk" | "runtime";
  onRecover: () => void;
}) {
  const label = kind === "chunk" ? "Couldn't load this panel" : "Something went wrong";
  const cta = kind === "chunk" ? "Reload" : "Retry";
  return (
    <div className="pointer-events-auto flex items-center gap-2 rounded border border-danger/40 bg-bg/90 px-3 py-2 text-xs text-text-muted">
      <span className="font-medium text-danger">{label}</span>
      <button
        type="button"
        onClick={onRecover}
        className="rounded bg-brand px-2 py-1 text-xs text-white hover:bg-brand/90"
      >
        {cta}
      </button>
    </div>
  );
}
