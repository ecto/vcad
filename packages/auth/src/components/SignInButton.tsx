import { useState } from "react";
import { UserCircle } from "@phosphor-icons/react";
import { useAuthStore } from "../stores/auth-store";
import { isAuthEnabled } from "../client";
import { AuthModal } from "./AuthModal";

interface SignInButtonProps {
  /** Optional class name */
  className?: string;
  /** Visual variant */
  variant?: "default" | "icon-text";
}

/**
 * Button that shows sign-in modal when clicked.
 * Hidden if user is already signed in or auth is disabled.
 */
export function SignInButton({ className, variant = "default" }: SignInButtonProps) {
  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  const loading = useAuthStore((s) => s.loading);
  const [showAuth, setShowAuth] = useState(false);

  // Hide if auth is disabled or the user has a permanent identity. Anon
  // sessions still show the button so the user can upgrade.
  if (!isAuthEnabled() || (user && !isAnonymous)) {
    return null;
  }

  // Don't render while loading initial auth state
  if (loading) {
    return null;
  }

  return (
    <>
      <button
        onClick={() => setShowAuth(true)}
        className={
          className ||
          (variant === "icon-text"
            ? "flex items-center gap-1.5 px-2 py-1.5 text-sm font-medium text-zinc-700 dark:text-zinc-300 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-lg transition-colors"
            : "px-3 py-1.5 text-sm font-medium text-zinc-700 dark:text-zinc-300 hover:text-zinc-900 dark:hover:text-zinc-100 hover:bg-zinc-100 dark:hover:bg-zinc-800 rounded-lg transition-colors")
        }
      >
        {variant === "icon-text" ? (
          <>
            <UserCircle size={16} />
            <span className="sr-only sm:not-sr-only">Sign in</span>
          </>
        ) : (
          "Sign in"
        )}
      </button>
      <AuthModal open={showAuth} onOpenChange={setShowAuth} />
    </>
  );
}
