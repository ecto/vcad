import { useEffect, useRef, useState } from "react";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Lock } from "@phosphor-icons/react/dist/ssr/Lock";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";
import {
  useDocumentStore,
  useUiStore,
  useSketchStore,
  useChatStore,
  useParticipantStore,
  AI_PARTICIPANT_ID,
  getSketchPlaneName,
} from "@vcad/core";
import { useAuthStore, useSyncStore } from "@vcad/auth";
import { useDrawingStore } from "@/stores/drawing-store";
import { useElectronicsStore } from "@/stores/electronics-store";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { invoke, isTauri } from "@/lib/tauri";
import { analytics } from "@/lib/analytics";
import { useCapabilities } from "@/lib/capabilities";

/**
 * Titlebar identity strip — filename, ambient save state, scope breadcrumb.
 *
 * Centered in the header row. Click the name to inline-rename (Enter commits,
 * Esc reverts). The save dot reuses the same state machine as StatusBar:
 *  local dirty → brand/pulse, cloud syncing → yellow/pulse, sync error → danger,
 *  otherwise muted "saved". In a read-only share session, a lock badge replaces
 *  the save dot.
 *
 * Scope crumbs are clickable shortcuts back out of the current mode (exit
 * sketch, leave drawing-mode, etc).
 */

export function DocTitle({ macOverlay }: { macOverlay?: boolean }) {
  const documentName = useDocumentStore((s) => s.documentName);
  const setDocumentName = useDocumentStore((s) => s.setDocumentName);
  const isDirty = useDocumentStore((s) => s.isDirty);
  const readOnlyShare = useUiStore((s) => s.readOnlyShare);

  const user = useAuthStore((s) => s.user);
  const isAnonymous = useAuthStore((s) => s.isAnonymous);
  const isSignedIn = !!user && !isAnonymous;
  const syncStatus = useSyncStore((s) => s.syncStatus);

  const sketchActive = useSketchStore((s) => s.active);
  const sketchPlane = useSketchStore((s) => s.plane);
  const requestSketchExit = useSketchStore((s) => s.requestExit);

  const drawingMode = useDrawingStore((s) => s.viewMode);
  const setDrawingMode = useDrawingStore((s) => s.setViewMode);

  const electronicsActive = useElectronicsStore((s) => s.active);
  const exitElectronics = useElectronicsStore((s) => s.exit);

  // AI presence — treat the chat assistant as a peer editor. "Streaming"
  // means an LLM pass is live; useChatHandler retires the participant shortly
  // after the turn ends to clear the viewport frustum.
  const chatStreaming = useChatStore((s) => s.streaming);
  const openChat = useChatStore((s) => s.setOpen);
  const aiParticipant = useParticipantStore((s) =>
    s.participants.get(AI_PARTICIPANT_ID),
  );

  // Local edit-mode — null means not editing, string means draft value.
  const [draft, setDraft] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Keep the native Tauri window title + dock tile in sync with the doc name.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (!isTauri()) return;
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        if (cancelled) return;
        await getCurrentWindow().setTitle(
          documentName && documentName !== "Untitled"
            ? `${documentName} — vcad`
            : "vcad",
        );
      } catch {
        // Not running under Tauri, or window API missing — ignore.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [documentName]);

  // Native macOS modified-dot. setDocumentEdited: paints the dot inside
  // the close traffic light — the standard signal for unsaved changes.
  // Read-only sessions are never "edited" from the OS's perspective even
  // if local diff state exists.
  useEffect(() => {
    if (!isTauri()) return;
    const edited = !!isDirty && !readOnlyShare;
    invoke<void>("set_document_edited", { edited }).catch(() => {
      // Mac-only command; silently no-op on Windows/Linux.
    });
  }, [isDirty, readOnlyShare]);

  // Select the whole name when edit mode opens so a single click + type
  // replaces it wholesale — the familiar Finder rename interaction.
  // Depending on `draft !== null` (not `draft` itself) keeps the effect from
  // re-firing on every keystroke — otherwise each new character re-selects
  // the text and gets replaced by the next one.
  const editing = draft !== null;
  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const startEdit = () => {
    if (readOnlyShare) return;
    setDraft(documentName);
  };
  const commitEdit = () => {
    if (draft === null) return;
    const next = draft.trim();
    if (next && next !== documentName) setDocumentName(next);
    setDraft(null);
  };
  const cancelEdit = () => setDraft(null);

  // Save-state visual — mirrors StatusBar's decision tree so the two
  // indicators never disagree. The StatusBar dot is gone once this lands.
  const saveIndicator: {
    dotClass: string;
    pulse: boolean;
    tooltip: string;
  } = isDirty
    ? {
        dotClass: "text-brand",
        pulse: true,
        tooltip: "Unsaved changes",
      }
    : isSignedIn && syncStatus === "syncing"
      ? {
          dotClass: "text-yellow-500",
          pulse: true,
          tooltip: "Syncing to cloud",
        }
      : isSignedIn && syncStatus === "error"
        ? {
            dotClass: "text-danger",
            pulse: false,
            tooltip: "Cloud sync failed",
          }
        : {
            dotClass: "text-text-muted/50",
            pulse: false,
            tooltip: isSignedIn ? "Saved · synced to cloud" : "Saved",
          };

  const nameDisplay = documentName || "Untitled";
  const nameIsDefault = !documentName || documentName === "Untitled";

  // ⌘-click path popover — Finder-style breadcrumb that slides down from
  // the title bar. Anchored to the proxy icon, dismissed on outside click
  // or Escape. The "path" is synthetic for now: cloud-synced docs read as
  // `~/vcad/<name>`, locals as `Untitled — vcad`. Rendered only inside
  // Tauri-mac, where the affordance is expected.
  const { tauri: inTauri, platform } = useCapabilities();
  const showProxy = inTauri && platform === "mac" && !sketchActive && drawingMode !== "2d" && !electronicsActive;
  const [pathOpen, setPathOpen] = useState(false);
  const pathRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!pathOpen) return;
    function onDocClick(e: MouseEvent) {
      if (!pathRef.current) return;
      if (!pathRef.current.contains(e.target as Node)) setPathOpen(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setPathOpen(false);
    }
    window.addEventListener("mousedown", onDocClick);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDocClick);
      window.removeEventListener("keydown", onKey);
    };
  }, [pathOpen]);

  const handleProxyClick = (e: React.MouseEvent) => {
    if (e.metaKey || e.ctrlKey) {
      e.preventDefault();
      setPathOpen((v) => !v);
    }
  };

  return (
    <div
      data-tauri-drag-region={macOverlay ? "false" : undefined}
      className={cn(
        "relative flex h-6 items-center gap-1.5 px-2 text-[11px] select-none",
        "max-w-[min(60vw,720px)]",
      )}
    >
      {/* Proxy icon — Finder-style document glyph. ⌘-click opens the path
          popover (vcad's "where does this live" affordance). Drag exports
          a virtual file ref the rest of the OS can drop into Finder. */}
      {showProxy && (
        <Tooltip content="⌘-click for path">
          <span
            className="proxy-icon shrink-0"
            draggable={!nameIsDefault}
            onClick={handleProxyClick}
            onDragStart={(e) => {
              e.dataTransfer.setData(
                "text/plain",
                nameIsDefault ? "Untitled.vcad" : `${documentName}.vcad`,
              );
              e.dataTransfer.effectAllowed = "copy";
            }}
            aria-label="Document proxy"
          >
            v
          </span>
        </Tooltip>
      )}

      {/* Filename — click to rename */}
      {draft !== null ? (
        <input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitEdit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitEdit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancelEdit();
            }
          }}
          className={cn(
            "h-5 min-w-[4ch] max-w-[32ch] px-1 text-[11px] font-medium",
            "bg-bg text-text border border-brand/60 outline-none",
            "rounded-sm",
          )}
          spellCheck={false}
        />
      ) : (
        <Tooltip content={readOnlyShare ? "Read-only share" : "Click to rename"}>
          <button
            type="button"
            onClick={startEdit}
            disabled={!!readOnlyShare}
            className={cn(
              "h-5 px-1 text-[11px] font-medium truncate max-w-[32ch] rounded-sm",
              "hover:bg-hover transition-colors appkit-spring outline-none",
              nameIsDefault ? "text-text-muted italic" : "text-text",
              readOnlyShare && "cursor-not-allowed opacity-80",
            )}
          >
            {nameDisplay}
          </button>
        </Tooltip>
      )}

      {/* Save state — swapped for a lock in read-only sessions */}
      {readOnlyShare ? (
        <Tooltip content="Read-only share — fork to edit">
          <span className="flex items-center gap-1 text-amber-500 text-[10px]">
            <Lock size={10} weight="fill" />
            <span className="uppercase tracking-wide">read-only</span>
          </span>
        </Tooltip>
      ) : (
        <Tooltip content={saveIndicator.tooltip}>
          <Circle
            size={6}
            weight="fill"
            className={cn(
              saveIndicator.dotClass,
              saveIndicator.pulse && "animate-pulse",
            )}
            aria-label={saveIndicator.tooltip}
          />
        </Tooltip>
      )}

      {/* Scope breadcrumb — only shows when out of doc root */}
      {sketchActive && (
        <ScopeCrumb
          onExit={() => {
            // Immediate exit only happens for an empty sketch; with segments
            // the confirmation in FeatureTree fires the abandon event instead.
            if (requestSketchExit()) analytics.sketchAbandoned("empty");
          }}
        >
          Sketch <span className="text-text-muted">on</span>{" "}
          {getSketchPlaneName(sketchPlane)}
        </ScopeCrumb>
      )}
      {!sketchActive && drawingMode === "2d" && (
        <ScopeCrumb onExit={() => setDrawingMode("3d")}>Drawing</ScopeCrumb>
      )}
      {!sketchActive && electronicsActive && (
        <ScopeCrumb onExit={() => exitElectronics()}>Electronics</ScopeCrumb>
      )}

      {/* AI presence — visible while the assistant is actively editing. */}
      {chatStreaming && aiParticipant && (
        <Tooltip content="Open chat">
          <button
            type="button"
            onClick={() => openChat(true)}
            className={cn(
              "ml-1 flex items-center gap-1 h-5 px-1.5 rounded-sm",
              "bg-brand/10 hover:bg-brand/20 transition-colors outline-none",
              "text-[10px] font-medium",
            )}
            style={{ color: aiParticipant.color }}
            aria-label={`${aiParticipant.name} is editing`}
          >
            <Sparkle size={9} weight="fill" className="animate-pulse" />
            <span>{aiParticipant.name}</span>
            <span className="text-text-muted">editing</span>
          </button>
        </Tooltip>
      )}

      {/* ⌘-click path popover — Finder-like breadcrumb panel. */}
      {pathOpen && (
        <div
          ref={pathRef}
          className="path-popover absolute left-1/2 top-[calc(100%+4px)] -translate-x-1/2 z-[80] min-w-[260px] px-3 py-2 text-[11px] rounded-md"
          role="dialog"
        >
          <div className="flex items-center gap-1 text-text-muted">
            <span className="proxy-icon" style={{ width: 12, height: 12, fontSize: 8 }}>v</span>
            <span>vcad</span>
            <CaretRight size={9} className="opacity-60" />
            <span>{isSignedIn ? "Cloud" : "Local"}</span>
            <CaretRight size={9} className="opacity-60" />
            <span className={cn("text-text", nameIsDefault && "italic")}>
              {nameDisplay}.vcad
            </span>
          </div>
          {isDirty && (
            <div className="mt-1 text-[10px] text-text-muted/80">
              <Circle size={5} weight="fill" className="inline mr-1 text-brand" />
              Unsaved changes
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ScopeCrumb({
  onExit,
  children,
}: {
  onExit: () => void;
  children: React.ReactNode;
}) {
  return (
    <>
      <CaretRight size={10} className="text-text-muted/60 shrink-0" />
      <Tooltip content="Click to exit this mode">
        <button
          type="button"
          onClick={onExit}
          className={cn(
            "h-5 px-1 text-[11px] text-text-muted rounded-sm",
            "hover:bg-hover hover:text-text transition-colors outline-none",
          )}
        >
          {children}
        </button>
      </Tooltip>
    </>
  );
}
