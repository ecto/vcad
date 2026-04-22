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
        const { isTauri } = await import("@/lib/tauri");
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

  // Select the whole name when edit mode opens so a single click + type
  // replaces it wholesale — the familiar Finder rename interaction.
  useEffect(() => {
    if (draft !== null) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [draft]);

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

  return (
    <div
      data-tauri-drag-region={macOverlay ? "false" : undefined}
      className={cn(
        "flex h-6 items-center gap-1.5 px-2 text-[11px] select-none",
        "max-w-[min(60vw,720px)]",
      )}
    >
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
              "hover:bg-hover transition-colors outline-none",
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
        <ScopeCrumb onExit={() => requestSketchExit()}>
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
