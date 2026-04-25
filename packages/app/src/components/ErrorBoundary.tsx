import { Component, type ReactNode } from "react";
import { t } from "@vcad/core";

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: React.ErrorInfo) {
    console.error("React error boundary caught error:", error);
    console.error("Component stack:", errorInfo.componentStack);

    // Log to PostHog for analytics (lazy — no-ops if not loaded yet)
    import("posthog-js").then(({ default: posthog }) => {
      posthog.capture("react_error_boundary", {
        error_message: error.message,
        error_name: error.name,
        error_stack: error.stack?.slice(0, 2000),
        component_stack: errorInfo.componentStack?.slice(0, 2000),
      });
    });
  }

  handleReload = () => {
    window.location.reload();
  };

  render() {
    if (this.state.hasError) {
      return (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-bg">
          <div className="flex max-w-md flex-col items-center gap-4 p-8 text-center">
            <div className="text-sm font-bold text-danger">{t("error.boundary.title")}</div>
            <div className="text-xs text-text-muted">
              {this.state.error?.message || t("error.boundary.unexpected")}
            </div>
            <button
              onClick={this.handleReload}
              className="mt-2 rounded bg-brand px-4 py-2 text-sm text-white hover:bg-brand/90"
            >
              {t("error.boundary.reload")}
            </button>
            <div className="mt-4 text-xs text-text-muted/60">
              {t("error.boundary.cache_hint")}
            </div>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
