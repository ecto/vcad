import { Component, type ReactNode } from "react";

/**
 * R3F-safe error boundary for the post-processing pipeline.
 *
 * The `@react-three/postprocessing` EffectComposer reads
 * `renderer.getContext().getContextAttributes().alpha` when wiring passes. On a
 * degraded/lost WebGL context — Safari under GPU pressure, tab restore, or "too
 * many live contexts" in automation/preview — `getContextAttributes()` returns
 * null and the composer throws, which would otherwise white-screen the whole
 * viewport. Here we catch it and render nothing, so the scene falls back to
 * rendering without AO/vignette instead of crashing.
 *
 * Renders `null` (not a DOM fallback) so it's valid inside the Canvas tree.
 */
export class PostProcessingBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: Error) {
    console.warn(
      "[viewport] post-processing disabled after EffectComposer error:",
      error?.message,
    );
  }

  render() {
    return this.state.failed ? null : this.props.children;
  }
}
