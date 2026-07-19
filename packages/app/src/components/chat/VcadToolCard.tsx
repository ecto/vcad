import { useState } from "react";
import { Wrench, Check, X, SpinnerGap, CaretDown } from "@phosphor-icons/react/dist/ssr";
import { useUiStore } from "@vcad/core";
import type { ToolCallInfo } from "@vcad/core";
import { Tool, ToolContent } from "@/components/ai-elements/tool";
import { CollapsibleTrigger } from "@/components/shadcn/collapsible";
import { CodeBlock } from "@/components/ai-elements/code-block";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// PartLink — inline clickable part reference. Rendered as a <span> (not a
// <button>) because it appears inside the CollapsibleTrigger, which is itself
// a <button> — and HTML forbids nested interactive content. role + keyboard
// handlers preserve the affordance for assistive tech.
// ---------------------------------------------------------------------------

function PartLink({ partId, name }: { partId: string; name: string }) {
  const select = (e: React.SyntheticEvent) => {
    e.stopPropagation();
    e.preventDefault();
    useUiStore.getState().select(partId);
  };
  return (
    <span
      role="button"
      tabIndex={0}
      onClick={select}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") select(e);
      }}
      className="inline-flex cursor-pointer items-center rounded px-1 bg-brand/10 text-brand hover:bg-brand/20 transition-colors font-medium"
    >
      {name}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Per-tool extra UI registry — adds custom widgets above the generic field
// grid for tools where display.fields alone isn't enough. Add new entries
// when their corresponding tools land; don't speculate.
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

const extraRenderers: Record<string, ExtraRenderer> = {
  set_material: (call) => <MaterialSwatch call={call} />,
};

// ---------------------------------------------------------------------------
// Claim chips — honesty as UX. Any solver/verification tool result that
// carries vcad.receipt/1 claims (or check_clearance's inline pass/verdict)
// renders each claim as a small status chip: Holds (verified pass),
// Provisional (predicted/surrogate pass — not yet verified against reality),
// Violated (fail), Unverifiable (the oracle couldn't check it — never a
// silent pass). Hover shows the claim's description.
// ---------------------------------------------------------------------------

type ChipStatus = "holds" | "provisional" | "violated" | "unverifiable";

interface ClaimChipInfo {
  status: ChipStatus;
  label: string;
  title: string;
}

const CHIP_STYLE: Record<ChipStatus, string> = {
  holds: "bg-success/10 text-success border-success/30",
  provisional: "bg-warning/10 text-warning border-warning/30",
  violated: "bg-error/10 text-error border-error/30",
  unverifiable: "bg-surface text-text-muted border-border",
};

const CHIP_TEXT: Record<ChipStatus, string> = {
  holds: "Holds",
  provisional: "Provisional",
  violated: "Violated",
  unverifiable: "Unverifiable",
};

function chipStatus(verdict: unknown, basis: unknown): ChipStatus {
  if (verdict === "fail") return "violated";
  if (verdict === "unverifiable") return "unverifiable";
  if (verdict !== "pass") return "unverifiable";
  return basis === "predicted" || basis === "surrogate" ? "provisional" : "holds";
}

/** Pull receipt claims out of a tool-result payload: `claim`, `claims`, or
 *  `receipt.claims`; check_clearance's inline pass/verdict synthesizes one. */
function extractClaims(call: ToolCallInfo): ClaimChipInfo[] {
  if (call.status !== "success" || typeof call.result !== "string") return [];
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(call.result) as Record<string, unknown>;
  } catch {
    return [];
  }
  const raw: Array<Record<string, unknown>> = [];
  const push = (c: unknown) => {
    if (c && typeof c === "object" && "verdict" in (c as object)) {
      raw.push(c as Record<string, unknown>);
    }
  };
  push(payload.claim);
  if (Array.isArray(payload.claims)) payload.claims.forEach(push);
  const receipt = payload.receipt as Record<string, unknown> | undefined;
  if (receipt && Array.isArray(receipt.claims)) receipt.claims.forEach(push);

  const chips: ClaimChipInfo[] = raw.map((c) => ({
    status: chipStatus(c.verdict, c.basis),
    label: typeof c.id === "string" ? c.id : "claim",
    title:
      typeof c.description === "string"
        ? c.description
        : typeof c.id === "string"
          ? c.id
          : "receipt claim",
  }));

  // check_clearance carries its verdict inline, not as a claim object.
  if (chips.length === 0 && call.name === "check_clearance" && "pass" in payload) {
    const label = typeof payload.label === "string" ? payload.label : "clearance";
    chips.push({
      status: payload.pass === true ? "holds" : "violated",
      label,
      title: `min distance ${payload.measured_mm} mm (required ${payload.required_mm} mm)`,
    });
  }
  return chips;
}

function ClaimChips({ call }: { call: ToolCallInfo }) {
  const chips = extractClaims(call);
  if (chips.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-1">
      {chips.map((chip, i) => (
        <span
          key={i}
          title={chip.title}
          className={cn(
            "inline-flex items-center gap-1 rounded-full border px-1.5 py-px text-[9px] font-medium",
            CHIP_STYLE[chip.status],
          )}
        >
          <span className="font-mono opacity-70">{chip.label}</span>
          {CHIP_TEXT[chip.status]}
        </span>
      ))}
    </div>
  );
}

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
      {/* Inline image preview — shown in the collapsed chip so a user doesn't
          have to expand the tool row to see a screenshot. */}
      {call.imageDataUrl && (
        <div className="border-t border-border p-1">
          <img
            src={call.imageDataUrl}
            alt={`${call.name} result`}
            className="block w-full rounded-sm"
          />
        </div>
      )}
      <ToolContent className="space-y-2 border-t border-border p-2 text-text">
        {extra}
        <ClaimChips call={call} />

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
