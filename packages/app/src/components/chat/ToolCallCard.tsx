import { useState } from "react";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { cn } from "@/lib/utils";
import { useUiStore } from "@vcad/core";
import type { ToolCallInfo } from "@vcad/core";

// ---------------------------------------------------------------------------
// PartLink — inline clickable part reference
// ---------------------------------------------------------------------------

function PartLink({ partId, name }: { partId: string; name: string }) {
  const handleClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    useUiStore.getState().select(partId);
  };
  return (
    <button
      onClick={handleClick}
      className="inline-flex items-center rounded px-1 bg-accent/10 text-accent hover:bg-accent/20 transition-colors font-medium"
    >
      {name}
    </button>
  );
}

// ---------------------------------------------------------------------------
// ChipSummary — the at-rest summary row
// ---------------------------------------------------------------------------

function ChipSummary({ call }: { call: ToolCallInfo }) {
  if (call.display?.summary && call.status !== "error") {
    return (
      <span className="flex-1 truncate">
        {call.display.summary.map((seg, i) =>
          seg.type === "text" ? (
            <span key={i}>{seg.text}</span>
          ) : (
            <PartLink key={i} partId={seg.partId} name={seg.name} />
          ),
        )}
      </span>
    );
  }
  if (call.status === "error" && typeof call.result === "string") {
    return (
      <span className="flex-1 truncate text-error">
        <span className="font-mono">{call.name}</span>
        <span className="ml-1 text-[9px]">{call.result}</span>
      </span>
    );
  }
  return <span className="font-mono text-text-muted truncate flex-1">{call.name}</span>;
}

// ---------------------------------------------------------------------------
// ChipDetail — the expanded detail pane
// ---------------------------------------------------------------------------

function ChipDetail({ call }: { call: ToolCallInfo }) {
  const [rawOpen, setRawOpen] = useState(false);
  const fields = call.display?.fields ?? [];
  const isError = call.status === "error";

  return (
    <div className="px-2 pb-2 border-t border-border">
      {fields.length > 0 && (
        <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5 text-[9px]">
          {fields.map((f, i) => (
            <div key={i} className="contents">
              <dt className="text-text-muted">{f.label}:</dt>
              <dd className="text-text font-mono">{f.value}</dd>
            </div>
          ))}
        </dl>
      )}
      {isError && typeof call.result === "string" && (
        <div className="mt-1">
          <div className="text-[9px] text-error font-medium">Error:</div>
          <pre className="text-[9px] text-error whitespace-pre-wrap break-all font-mono leading-relaxed">
            {call.result}
          </pre>
        </div>
      )}
      <div className="mt-1 flex items-center gap-1">
        <button
          onClick={() => setRawOpen((r) => !r)}
          className="text-[9px] text-text-muted hover:text-text transition-colors"
        >
          {rawOpen ? "hide raw" : "raw"}
        </button>
        {call.duration != null && (
          <span className="ml-auto text-[9px] text-text-muted">
            {call.duration < 1 ? "<1" : call.duration.toFixed(0)}ms
          </span>
        )}
      </div>
      {rawOpen && (
        <>
          <pre className="mt-1 text-[9px] text-text-muted whitespace-pre-wrap break-all font-mono leading-relaxed">
            {JSON.stringify(call.args, null, 2)}
          </pre>
          {call.result !== undefined && !isError && (
            <>
              <div className="mt-1 text-[9px] text-text-muted font-medium">Result:</div>
              <pre className="text-[9px] text-text-muted whitespace-pre-wrap break-all font-mono leading-relaxed">
                {typeof call.result === "string" ? call.result : JSON.stringify(call.result, null, 2)}
              </pre>
            </>
          )}
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// ToolCallCard — the outer chip
// ---------------------------------------------------------------------------

export function ToolCallCard({ call }: { call: ToolCallInfo }) {
  const [expanded, setExpanded] = useState(false);

  const statusIcon =
    call.status === "success" ? (
      <Check size={10} className="text-success shrink-0" />
    ) : call.status === "error" ? (
      <XCircle size={10} className="text-error shrink-0" />
    ) : (
      <SpinnerGap size={10} className="animate-spin text-text-muted shrink-0" />
    );

  return (
    <div className="mt-1 border border-border bg-bg rounded text-[10px]">
      <button
        onClick={() => setExpanded((e) => !e)}
        className="flex w-full items-center gap-1.5 px-2 py-1 text-left hover:bg-hover transition-colors"
      >
        {statusIcon}
        <ChipSummary call={call} />
        <CaretRight
          size={10}
          className={cn(
            "text-text-muted transition-transform shrink-0",
            expanded && "rotate-90",
          )}
        />
      </button>
      {expanded && <ChipDetail call={call} />}
    </div>
  );
}
