import { useEffect, useRef } from "react";
import {
  useDocumentStore,
  useUiStore,
  useSketchStore,
  type SidebarPane,
} from "@vcad/core";

const INSPECT_VALUES = ["scene"] as const;
type InspectValue = (typeof INSPECT_VALUES)[number];
function isInspectValue(v: string | null): v is InspectValue {
  return v != null && (INSPECT_VALUES as readonly string[]).includes(v);
}

/**
 * Returns the pathname the URL should carry for a given `documentId`, or null
 * to leave the pathname alone. We only own the `/~<id>` (and legacy `/d/<id>`)
 * and `/` prefixes — any other path (share token, public slug, profile page)
 * is another subsystem's to manage.
 */
function desiredPathname(documentId: string | null): string | null {
  const path = window.location.pathname;
  const ownedByDocSync =
    path === "/" || path.startsWith("/~") || /^\/d\//.test(path);
  if (!ownedByDocSync) return null;
  return documentId ? `/~${documentId}` : "/";
}

/**
 * Mirrors a small slice of UI state to and from the URL query string so that
 * a refresh, deep-link, or browser back/forward navigates as the user expects.
 *
 * Mirrored fields:
 *   ?select=<partId[,partId…]>   active selection
 *   ?pane=tree|inspector          left sidebar pane
 *   ?sketch=<id>                  active sketch (presence only)
 *
 * The hook is intentionally one-way per direction per tick: store changes
 * `replaceState` the URL, popstate (back/forward) re-applies the URL onto
 * stores, and a guard ref prevents the two from chasing each other.
 */
export function useUrlSync() {
  const documentId = useDocumentStore((s) => s.documentId);
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const sidebarPane = useUiStore((s) => s.sidebarPane);
  const inspectorTarget = useUiStore((s) => s.inspectorTarget);
  const sketchActive = useSketchStore((s) => s.active);
  const applyingFromUrl = useRef(false);

  // ── Apply URL → stores on mount and on browser back/forward ────────────
  useEffect(() => {
    function applyFromUrl() {
      const params = new URLSearchParams(window.location.search);
      applyingFromUrl.current = true;
      try {
        const select = params.get("select");
        if (select) {
          const ids = select.split(",").filter(Boolean);
          useUiStore.getState().selectMultiple(ids);
        } else {
          useUiStore.getState().clearSelection();
        }

        const pane = params.get("pane");
        if (pane === "tree" || pane === "inspector") {
          useUiStore.getState().setSidebarPane(pane);
        }

        const inspect = params.get("inspect");
        if (isInspectValue(inspect)) {
          useUiStore.getState().setInspectorTarget({ kind: inspect });
        } else {
          useUiStore.getState().setInspectorTarget(null);
        }
      } finally {
        applyingFromUrl.current = false;
      }
    }

    applyFromUrl();
    window.addEventListener("popstate", applyFromUrl);
    return () => window.removeEventListener("popstate", applyFromUrl);
  }, []);

  // ── Push store → URL whenever the mirrored slice changes ───────────────
  useEffect(() => {
    if (applyingFromUrl.current) return;

    const params = new URLSearchParams(window.location.search);

    if (selectedPartIds.size > 0) {
      params.set("select", Array.from(selectedPartIds).join(","));
    } else {
      params.delete("select");
    }

    // Only encode the pane when it's the non-default value, so URLs stay clean.
    const paneValue: SidebarPane = sidebarPane;
    if (paneValue === "inspector") {
      params.set("pane", "inspector");
    } else {
      params.delete("pane");
    }

    if (inspectorTarget) {
      params.set("inspect", inspectorTarget.kind);
    } else {
      params.delete("inspect");
    }

    if (sketchActive) {
      params.set("sketch", "1");
    } else {
      params.delete("sketch");
    }

    const next = params.toString();
    // The pathname is either the `/~<id>` we want (local doc), or we leave
    // it alone (share token / public slug / profile page).
    const pathname =
      desiredPathname(documentId) ?? window.location.pathname;
    const url = `${pathname}${next ? "?" + next : ""}${window.location.hash}`;
    if (url !== window.location.pathname + window.location.search + window.location.hash) {
      window.history.replaceState(null, "", url);
    }
  }, [documentId, selectedPartIds, sidebarPane, sketchActive, inspectorTarget]);
}
