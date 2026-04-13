import { useState } from "react";
import { Wrench, Check, X, SpinnerGap, CaretDown, Download } from "@phosphor-icons/react/dist/ssr";
import { useUiStore } from "@vcad/core";
import type { ToolCallInfo } from "@vcad/core";
import { Tool, ToolContent } from "@/components/ai-elements/tool";
import { CollapsibleTrigger } from "@/components/shadcn/collapsible";
import { CodeBlock } from "@/components/ai-elements/code-block";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// PartLink — inline clickable part reference
// ---------------------------------------------------------------------------

function PartLink({ partId, name }: { partId: string; name: string }) {
  return (
    <button
      onClick={(e) => {
        e.stopPropagation();
        useUiStore.getState().select(partId);
      }}
      className="inline-flex items-center rounded px-1 bg-accent/10 text-accent hover:bg-accent/20 transition-colors font-medium"
    >
      {name}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Per-tool extra UI registry — adds custom widgets above the generic field grid.
// Most tools are well-served by display.fields alone; this is for the few that
// benefit from a richer affordance (color swatch, download button, etc.).
// ---------------------------------------------------------------------------

type ExtraRenderer = (call: ToolCallInfo) => React.ReactNode;

function MaterialSwatch({ call }: { call: ToolCallInfo }) {
  const color =
    typeof call.args.color === "string"
      ? call.args.color
      : typeof call.args.hex === "string"
        ? call.args.hex
        : null;
  if (!color) return null;
  return (
    <div className="flex items-center gap-2">
      <div
        className="h-5 w-5 rounded border border-border shrink-0"
        style={{ background: color }}
        aria-label={`Color ${color}`}
      />
      <code className="text-[10px] text-text-muted">{color}</code>
    </div>
  );
}

function ReadStats({ call }: { call: ToolCallInfo }) {
  // The inspect/read result is a JSON string with bbox/volume/area when the
  // model asked for measurements. Try to parse and render as a stats grid.
  if (typeof call.result !== "string") return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(call.result);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const obj = parsed as Record<string, unknown>;
  const stats: Array<[string, string]> = [];
  if (typeof obj.volume === "number") stats.push(["Volume", `${obj.volume.toFixed(2)} mm³`]);
  if (typeof obj.area === "number") stats.push(["Surface", `${obj.area.toFixed(2)} mm²`]);
  if (Array.isArray(obj.bbox) && obj.bbox.length === 6 && obj.bbox.every((v) => typeof v === "number")) {
    const b = obj.bbox as [number, number, number, number, number, number];
    stats.push(["Size", `${(b[3] - b[0]).toFixed(1)} × ${(b[4] - b[1]).toFixed(1)} × ${(b[5] - b[2]).toFixed(1)} mm`]);
  }
  if (Array.isArray(obj.center) && obj.center.length === 3 && obj.center.every((v) => typeof v === "number")) {
    const c = obj.center as [number, number, number];
    stats.push(["Center", `(${c[0].toFixed(1)}, ${c[1].toFixed(1)}, ${c[2].toFixed(1)})`]);
  }
  if (stats.length === 0) return null;
  return (
    <div className="rounded border border-border bg-bg p-2">
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[10px]">
        {stats.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-text-muted">{k}</dt>
            <dd className="text-text font-mono">{v}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

function ExportDownload({ call }: { call: ToolCallInfo }) {
  // The export tool returns a result string; if it contains a data URL or a
  // file path, surface a download button. Otherwise no-op.
  if (call.status !== "success" || typeof call.result !== "string") return null;
  const dataUrlMatch = call.result.match(/(data:[^\s)]+|blob:[^\s)]+|https?:\/\/[^\s)]+\.(?:stl|glb|step|stp|dxf))/i);
  if (!dataUrlMatch) return null;
  const url = dataUrlMatch[1];
  const filename = (typeof call.args.filename === "string" && call.args.filename) || "export";
  return (
    <a
      href={url}
      download={filename}
      className="inline-flex items-center gap-1.5 rounded border border-border bg-bg px-2 py-1 text-[10px] text-text hover:bg-hover transition-colors"
    >
      <Download size={11} />
      Download {filename}
    </a>
  );
}

const extraRenderers: Record<string, ExtraRenderer> = {
  set_material: (call) => <MaterialSwatch call={call} />,
  read: (call) => <ReadStats call={call} />,
  // Vcad doesn't currently route exports through the tool registry, but if it
  // did, this slot would handle it.
  export_cad: (call) => <ExportDownload call={call} />,
};

// ---------------------------------------------------------------------------
// Title — the at-rest summary line shown next to the icon
// ---------------------------------------------------------------------------

function Title({ call }: { call: ToolCallInfo }) {
  if (call.display?.summary && call.status !== "error") {
    return (
      <span className="truncate text-left text-[11px] text-text">
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
      <span className="truncate text-left text-[11px] text-error">
        <span className="font-mono">{call.name}</span>
        <span className="ml-1 text-[9px]">{call.result}</span>
      </span>
    );
  }
  return <span className="truncate text-left font-mono text-[11px] text-text-muted">{call.name}</span>;
}

// ---------------------------------------------------------------------------
// VcadToolCard — vcad-shaped wrapper around AI Elements' Tool collapsible
// ---------------------------------------------------------------------------

export function VcadToolCard({ call }: { call: ToolCallInfo }) {
  const [rawOpen, setRawOpen] = useState(false);
  const fields = call.display?.fields ?? [];
  const isError = call.status === "error";
  const extra = extraRenderers[call.name]?.(call);

  const StatusIcon =
    call.status === "success" ? (
      <Check size={11} className="text-success shrink-0" />
    ) : call.status === "error" ? (
      <X size={11} className="text-error shrink-0" />
    ) : (
      <SpinnerGap size={11} className="animate-spin text-text-muted shrink-0" />
    );

  return (
    <Tool className="mb-1.5 rounded border border-border bg-bg">
      <CollapsibleTrigger className="group/trigger flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-hover transition-colors">
        <Wrench size={11} className="text-text-muted shrink-0" />
        {StatusIcon}
        <Title call={call} />
        <CaretDown
          size={10}
          className="ml-auto text-text-muted shrink-0 transition-transform group-data-[state=open]/trigger:rotate-180"
        />
      </CollapsibleTrigger>
      <ToolContent className="space-y-2 border-t border-border p-2 text-text">
        {extra}

        {fields.length > 0 && (
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[10px]">
            {fields.map((f, i) => (
              <div key={i} className="contents">
                <dt className="text-text-muted">{f.label}</dt>
                <dd className="text-text font-mono break-all">{f.value}</dd>
              </div>
            ))}
          </dl>
        )}

        {isError && typeof call.result === "string" && (
          <div className="rounded bg-error/10 p-2">
            <div className="text-[10px] font-medium text-error mb-0.5">Error</div>
            <pre className="whitespace-pre-wrap break-all font-mono text-[10px] text-error leading-relaxed">
              {call.result}
            </pre>
          </div>
        )}

        <div className="flex items-center gap-2 text-[9px] text-text-muted">
          <button
            onClick={() => setRawOpen((r) => !r)}
            className="hover:text-text transition-colors"
          >
            {rawOpen ? "hide raw" : "raw"}
          </button>
          {call.duration != null && (
            <span className="ml-auto font-mono">
              {call.duration < 1 ? "<1" : call.duration.toFixed(0)}ms
            </span>
          )}
        </div>

        {rawOpen && (
          <div className={cn("space-y-1.5 rounded bg-surface p-2")}>
            <div>
              <div className="text-[9px] uppercase tracking-wide text-text-muted mb-1">Input</div>
              <CodeBlock code={JSON.stringify(call.args, null, 2)} language="json" />
            </div>
            {call.result !== undefined && !isError && (
              <div>
                <div className="text-[9px] uppercase tracking-wide text-text-muted mb-1">Result</div>
                <CodeBlock
                  code={typeof call.result === "string" ? call.result : JSON.stringify(call.result, null, 2)}
                  language="json"
                />
              </div>
            )}
          </div>
        )}
      </ToolContent>
    </Tool>
  );
}
