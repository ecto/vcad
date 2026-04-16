import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { useUiStore } from "@vcad/core";
import { cn } from "@/lib/utils";

/**
 * Thin bar rendered above the viewport when the app is in a read-only
 * share session. Fires the `vcad:fork-prompt` event when the user clicks
 * "Sign in to fork" — same event ForkPromptModal listens for.
 */
export function ReadOnlyBanner() {
  const readOnly = useUiStore((s) => s.readOnlyShare);
  if (!readOnly) return null;

  const handleForkClick = () => {
    window.dispatchEvent(
      new CustomEvent("vcad:fork-prompt", { detail: readOnly }),
    );
  };

  return (
    <div
      className={cn(
        "flex items-center justify-center gap-2 px-4 py-1.5",
        "bg-brand/10 border-b border-brand/20 text-[11px] text-text",
      )}
      role="status"
      aria-live="polite"
    >
      <Eye size={13} className="text-brand shrink-0" />
      <span>
        Viewing <span className="font-medium">{readOnly.docName}</span> (read-only)
      </span>
      <span className="text-text-muted">·</span>
      <button
        type="button"
        onClick={handleForkClick}
        className="text-brand hover:underline focus:outline-none"
      >
        Sign in to fork
      </button>
    </div>
  );
}
