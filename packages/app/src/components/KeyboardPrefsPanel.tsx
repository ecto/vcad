/**
 * Keyboard preferences panel.
 *
 * Shows every command in the shared registry grouped by category, with
 * the effective chord rendered using platform-appropriate glyphs. Each
 * row's chord is click-to-rebind: clicking the chord button puts the row
 * into "capture" mode, listens for the next keypress, and writes the
 * binding through `useKeybindingPrefs`. Conflicts (two commands sharing
 * a chord in the current mode) are flagged inline.
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import {
  chordFromEvent,
  formatChord,
  isMac,
  type Chord,
  type KeybindingCommandView,
} from "@vcad/core";
import { cn } from "@/lib/utils";
import { useKeybindingPrefs } from "@/hooks/useKeybindingPrefs";

const CATEGORY_ORDER = [
  "file",
  "edit",
  "create",
  "modify",
  "view",
  "tools",
  "help",
] as const;

const CATEGORY_LABELS: Record<string, string> = {
  file: "File",
  edit: "Edit",
  create: "Create",
  modify: "Modify",
  view: "View",
  tools: "Tools",
  help: "Help",
};

interface KeyboardPrefsPanelProps {
  className?: string;
}

export function KeyboardPrefsPanel({ className }: KeyboardPrefsPanelProps) {
  const { registry, commands, conflicts, setBinding, resetAll } =
    useKeybindingPrefs("Normal");
  const [filter, setFilter] = useState("");
  const [capturingId, setCapturingId] = useState<string | null>(null);

  const platform = isMac() ? "mac" : "pc";

  // Build a chord-key → command-ids map so each row can flag conflicts.
  const conflictByCmdId = useMemo(() => {
    const out = new Map<string, string[]>();
    for (const { ids } of conflicts) {
      for (const id of ids) {
        out.set(
          id,
          ids.filter((other) => other !== id),
        );
      }
    }
    return out;
  }, [conflicts]);

  // Filter + group commands.
  const grouped = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const filtered = commands.filter((cmd) => {
      if (!q) return true;
      if (cmd.label.toLowerCase().includes(q)) return true;
      if (cmd.id.toLowerCase().includes(q)) return true;
      return cmd.keywords.some((k) => k.toLowerCase().includes(q));
    });
    const out = new Map<string, KeybindingCommandView[]>();
    for (const cat of CATEGORY_ORDER) out.set(cat, []);
    for (const cmd of filtered) {
      const cat = cmd.category ?? "tools";
      if (!out.has(cat)) out.set(cat, []);
      out.get(cat)!.push(cmd);
    }
    return out;
  }, [commands, filter]);

  // Capture the next keypress when a row is in rebind mode. Listens in
  // capture phase so the global dispatcher doesn't intercept.
  useEffect(() => {
    if (!capturingId) return;
    const handler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setCapturingId(null);
        return;
      }
      const chord = chordFromEvent(e);
      if (!chord) return;
      setBinding(capturingId, chord);
      setCapturingId(null);
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, [capturingId, setBinding]);

  if (!registry) {
    return (
      <div className={cn("p-4 text-xs text-text-muted", className)}>
        Loading keybinding registry…
      </div>
    );
  }

  return (
    <div className={cn("flex flex-col h-full min-h-0", className)}>
      {/* Toolbar: search + reset all */}
      <div className="flex items-center gap-2 p-3 border-b border-border">
        <div className="relative flex-1">
          <MagnifyingGlass
            size={12}
            className="absolute left-2 top-1/2 -translate-y-1/2 text-text-muted"
          />
          <input
            type="text"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Search commands…"
            className={cn(
              "w-full h-7 pl-7 pr-2 bg-bg border border-border",
              "text-xs text-text placeholder-text-muted/60",
              "focus:outline-none focus:border-brand/60",
            )}
          />
        </div>
        <button
          onClick={resetAll}
          className={cn(
            "h-7 px-3 text-[11px] text-text-muted hover:text-text",
            "border border-border hover:bg-hover",
          )}
        >
          Reset all
        </button>
      </div>

      {/* Command list */}
      <div className="flex-1 min-h-0 overflow-y-auto">
        {CATEGORY_ORDER.map((cat) => {
          const items = grouped.get(cat) ?? [];
          if (items.length === 0) return null;
          return (
            <div key={cat}>
              <div
                className={cn(
                  "sticky top-0 z-10 px-3 py-1.5",
                  "text-[10px] font-bold uppercase tracking-wider",
                  "text-text-muted bg-surface border-b border-border/60",
                )}
              >
                {CATEGORY_LABELS[cat] ?? cat}
              </div>
              {items.map((cmd) => (
                <CommandRow
                  key={cmd.id}
                  command={cmd}
                  platform={platform}
                  capturing={capturingId === cmd.id}
                  conflictWith={conflictByCmdId.get(cmd.id)}
                  onStartCapture={() => setCapturingId(cmd.id)}
                  onClear={() => setBinding(cmd.id, null)}
                />
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

interface CommandRowProps {
  command: KeybindingCommandView;
  platform: "mac" | "pc";
  capturing: boolean;
  conflictWith: string[] | undefined;
  onStartCapture: () => void;
  onClear: () => void;
}

function CommandRow({
  command,
  platform,
  capturing,
  conflictWith,
  onStartCapture,
  onClear,
}: CommandRowProps) {
  const ref = useRef<HTMLDivElement>(null);

  // Scroll into view when capture starts so the user can see what they're
  // editing if the row was below the fold.
  useEffect(() => {
    if (capturing) ref.current?.scrollIntoView({ block: "nearest" });
  }, [capturing]);

  const effective = command.effective_chord;
  const isOverridden =
    !!command.default_chord &&
    JSON.stringify(command.default_chord) !== JSON.stringify(effective);
  const isCleared = command.default_chord !== null && effective === null;

  return (
    <div
      ref={ref}
      className={cn(
        "flex items-center gap-2 px-3 py-1.5 border-b border-border/30",
        "hover:bg-hover/40",
        capturing && "bg-brand/10",
      )}
    >
      <div className="flex-1 min-w-0">
        <div className="text-xs text-text truncate">{command.label}</div>
        {conflictWith && conflictWith.length > 0 && (
          <div className="flex items-center gap-1 text-[10px] text-amber-400 mt-0.5">
            <Warning size={10} weight="fill" />
            <span>conflicts with {conflictWith.join(", ")}</span>
          </div>
        )}
      </div>

      <button
        onClick={onStartCapture}
        className={cn(
          "h-6 min-w-[80px] px-2 font-mono text-[11px]",
          "border border-border bg-bg hover:bg-hover",
          "text-text",
          capturing && "border-brand text-brand",
          isOverridden && "border-brand/40",
        )}
      >
        {capturing ? (
          <span className="text-text-muted">Press a key…</span>
        ) : effective ? (
          formatChord(effective as Chord, platform)
        ) : isCleared ? (
          <span className="text-text-muted/60">disabled</span>
        ) : (
          <span className="text-text-muted/60">unbound</span>
        )}
      </button>

      {(isOverridden || isCleared) && (
        <button
          onClick={onClear}
          title="Clear binding"
          className="p-1 text-text-muted hover:text-text"
        >
          <X size={11} />
        </button>
      )}
      {isOverridden && command.default_chord && (
        <button
          onClick={() => onClear()}
          title={`Reset to ${formatChord(command.default_chord as Chord, platform)}`}
          className="p-1 text-text-muted hover:text-text"
        >
          <ArrowCounterClockwise size={11} />
        </button>
      )}
    </div>
  );
}
