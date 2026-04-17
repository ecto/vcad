import { useState, type ReactNode, type MouseEvent } from "react";
import { useAuthStore } from "../stores/auth-store";
import { isAuthEnabled } from "../client";
import { AuthModal } from "./AuthModal";
import type { GatedFeature } from "../hooks/useRequireAuth";

interface FeatureGateProps {
  /** The feature being gated */
  feature: GatedFeature;
  /** Content to render when user has access */
  children: ReactNode;
  /** Optional content to render when gated (defaults to children with click handler) */
  fallback?: ReactNode;
  /** Render as specific element type (default: div) */
  as?: "div" | "span" | "button";
  /** Additional class name for the wrapper */
  className?: string;
}

/**
 * Component that gates content behind authentication.
 * When user is not signed in, clicking the content shows the auth modal.
 *
 * @example
 * ```tsx
 * <FeatureGate feature="quotes">
 *   <QuotePanel />
 * </FeatureGate>
 * ```
 *
 * @example
 * ```tsx
 * <FeatureGate
 *   feature="step-export"
 *   fallback={<button disabled>Export STEP (Sign in required)</button>}
 * >
 *   <button onClick={exportStep}>Export STEP</button>
 * </FeatureGate>
 * ```
 */
export function FeatureGate({
  feature,
  children,
  fallback,
  as: Component = "div",
  className,
}: FeatureGateProps) {
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  const [showAuth, setShowAuth] = useState(false);

  // If auth not configured (self-hosted), allow all features
  if (!isAuthEnabled()) {
    return <>{children}</>;
  }

  // Anonymous Supabase sessions don't count as signed-in for gated features.
  if (user && !isAnonymous) {
    return <>{children}</>;
  }

  // User not signed in - show gated UI
  const handleClick = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setShowAuth(true);
  };

  return (
    <>
      <Component
        onClick={handleClick}
        className={className}
        style={{ cursor: "pointer" }}
      >
        {fallback ?? children}
      </Component>
      <AuthModal open={showAuth} onOpenChange={setShowAuth} feature={feature} />
    </>
  );
}
