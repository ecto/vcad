import { useState, useCallback, useRef, useEffect } from "react";
import { useDocumentStore, useEngineStore } from "@vcad/core";
import { parseVcadFile } from "@vcad/core";
import { useNotificationStore } from "@/stores/notification-store";

interface LoonEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Code editor panel for loon source. Changes re-evaluate and update the viewport.
 * MVP: simple textarea with monospace font + live eval (debounced).
 */
export function LoonEditor({ open, onOpenChange }: LoonEditorProps) {
  const loonSource = useDocumentStore((s) => s.loonSource);
  const [localSource, setLocalSource] = useState(loonSource ?? "");
  const [error, setError] = useState<string | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Sync when external loonSource changes (e.g. file load)
  useEffect(() => {
    if (loonSource !== null) {
      setLocalSource(loonSource);
      setError(null);
    }
  }, [loonSource]);

  const evalAndLoad = useCallback((source: string) => {
    const engine = useEngineStore.getState().engine;
    if (!engine) return;

    try {
      const evalLoon = (s: string) => {
        const doc = engine.evalVcadSource(s);
        if (!doc) throw new Error("Loon evaluation not supported");
        return JSON.stringify(doc);
      };
      const vcadFile = parseVcadFile(source, evalLoon);
      useDocumentStore.getState().loadDocument(vcadFile);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLTextAreaElement>) => {
      const value = e.target.value;
      setLocalSource(value);

      // Debounce eval
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        evalAndLoad(value);
      }, 300);
    },
    [evalAndLoad],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Cmd/Ctrl+Enter: immediate eval
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        if (debounceRef.current) clearTimeout(debounceRef.current);
        evalAndLoad(localSource);
      }
      // Tab inserts two spaces
      if (e.key === "Tab") {
        e.preventDefault();
        const ta = e.currentTarget;
        const start = ta.selectionStart;
        const end = ta.selectionEnd;
        const val = ta.value;
        ta.value = val.substring(0, start) + "  " + val.substring(end);
        ta.selectionStart = ta.selectionEnd = start + 2;
        setLocalSource(ta.value);
      }
    },
    [localSource, evalAndLoad],
  );

  if (!open) return null;

  return (
    <div className="fixed right-0 top-0 z-40 flex h-full w-[400px] flex-col border-l border-border bg-bg shadow-lg">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-border px-3 py-2">
        <span className="text-sm font-medium text-text">Loon Editor</span>
        <button
          onClick={() => onOpenChange(false)}
          className="rounded p-1 text-text-muted hover:bg-bg-hover hover:text-text"
          title="Close"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M18 6L6 18M6 6l12 12" />
          </svg>
        </button>
      </div>

      {/* Editor */}
      <textarea
        value={localSource}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        spellCheck={false}
        className="flex-1 resize-none bg-bg p-3 font-mono text-xs leading-relaxed text-text outline-none"
        placeholder={"; Write loon source here\n[cube 20.0 20.0 20.0]"}
      />

      {/* Error bar */}
      {error && (
        <div className="border-t border-danger/30 bg-danger/10 px-3 py-2 text-xs text-danger">
          {error}
        </div>
      )}

      {/* Footer */}
      <div className="border-t border-border px-3 py-1.5 text-[10px] text-text-muted">
        {loonSource !== null ? "Loon document" : "No loon source"} · Cmd+Enter to eval
      </div>
    </div>
  );
}
