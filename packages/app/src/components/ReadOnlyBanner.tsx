import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { useUiStore, t, tFmt } from "@vcad/core";
import { useLocaleStore } from "@/stores/locale-store";
import { cn } from "@/lib/utils";

/**
 * Thin bar rendered above the viewport when the app is in a read-only
 * share session. Fires the `vcad:fork-prompt` event when the user clicks
 * "Sign in to fork" — same event ForkPromptModal listens for.
 */
export function ReadOnlyBanner() {
  const readOnly = useUiStore((s) => s.readOnlyShare);
  useLocaleStore((s) => s.locale);
  if (!readOnly) return null;

  const handleForkClick = () => {
    window.dispatchEvent(
      new CustomEvent("vcad:fork-prompt", { detail: readOnly }),
    );
  };

  // Split the formatted banner around {name} so the document name keeps its
  // bold styling. The marker token is unlikely to appear in any translation.
  const MARKER = "__DOCNAME__";
  const parts = tFmt("banner.readonly.viewing", { name: MARKER }).split(MARKER);
  const before = parts[0] ?? "";
  const after = parts[1] ?? "";

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
        {before}
        <span className="font-medium">{readOnly.docName}</span>
        {after}
      </span>
      <span className="text-text-muted">·</span>
      <button
        type="button"
        onClick={handleForkClick}
        className="text-brand hover:underline focus:outline-none"
      >
        {t("banner.readonly.fork")}
      </button>
    </div>
  );
}
