import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCapabilities } from "@/lib/capabilities";

const INTERACTIVE_SELECTOR =
  "[data-tauri-drag-region='false'],button,input,textarea,select,a,[role='button'],[role='menuitem'],[contenteditable='true'],[contenteditable='']";

const DRAG_REGION_SELECTOR =
  "[data-tauri-drag-region]:not([data-tauri-drag-region='false'])";

/**
 * Document-level mousedown listener that initiates a native window drag when
 * the user clicks on any element inside a `data-tauri-drag-region` ancestor —
 * including elements rendered through React portals (modals, popovers).
 *
 * Why a global listener: Tauri v2 with `transparent: true` + `macOSPrivateApi`
 * has a known regression where the HTML attribute alone fails to start the
 * drag, and `startDragging()` must be called *synchronously* from the
 * mousedown handler (no awaited dynamic import). This hook satisfies both.
 */
export function useNativeWindowDrag(): void {
  const { tauri, platform } = useCapabilities();
  const enabled = tauri && platform === "mac";

  useEffect(() => {
    if (!enabled) return;
    const onDown = (e: MouseEvent) => {
      if (e.button !== 0) return;
      const target = e.target as HTMLElement | null;
      if (!target) return;
      if (!target.closest(DRAG_REGION_SELECTOR)) return;
      if (target.closest(INTERACTIVE_SELECTOR)) return;
      getCurrentWindow().startDragging().catch(() => {});
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [enabled]);
}
