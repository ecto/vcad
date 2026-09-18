/**
 * The operations the outline implies, in the order they run.
 *
 * Every row is a cut the part actually has — a pilot, an opening, the profile
 * that frees it — rather than a rectangle typed in by hand. The row is flagged
 * where a fault lands: the oracle reports a *place*, and a place inside one
 * operation's sweep is that operation's problem.
 */
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { WarningCircle } from "@phosphor-icons/react/dist/ssr/WarningCircle";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { ArrowUp } from "@phosphor-icons/react/dist/ssr/ArrowUp";
import { ArrowDown } from "@phosphor-icons/react/dist/ssr/ArrowDown";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { EyeSlash } from "@phosphor-icons/react/dist/ssr/EyeSlash";
import { cn } from "@/lib/utils";
import {
  findingsOf,
  useCamJobStore,
  type CamJobOperation,
} from "@/stores/cam-job-store";
import { NumberField } from "./NumberField";

/** What the kernel calls each kind, in the words on the machine. */
const KIND_LABELS: Record<string, string> = {
  helical_bore: "Helical bore",
  contour_inside: "Cut out",
  pocket: "Pocket",
  contour_outside: "Outside profile",
  face: "Face",
  drill: "Drill",
};

export function OperationList() {
  const operations = useCamJobStore((s) => s.operations);
  const outline = useCamJobStore((s) => s.outline);
  const result = useCamJobStore((s) => s.result);
  const highlighted = useCamJobStore((s) => s.highlightedOpIndex);
  const updateOperation = useCamJobStore((s) => s.updateOperation);
  const moveOperation = useCamJobStore((s) => s.moveOperation);
  const setHighlightedOp = useCamJobStore((s) => s.setHighlightedOp);

  if (!outline) {
    return (
      <p className="text-xs text-text-muted">
        Take the contour from the part first — the operations are the cuts that
        outline implies.
      </p>
    );
  }
  if (operations.length === 0) {
    return <p className="text-xs text-text-muted">This section has nothing to cut.</p>;
  }

  // A fault is placed on the operation whose request index it landed in. The
  // tool checks carry `op_index` directly; the verification findings carry a
  // place, and the ranges say whose sweep that is.
  const findings = result ? findingsOf(result) : { blockers: [], warnings: [] };
  const opIndexOfRow = (row: number): number | undefined => {
    // A helical-bore row is several kernel operations, so the row owns a span.
    let index = 0;
    for (let i = 0; i < operations.length; i++) {
      const op = operations[i];
      if (!op?.enabled) continue;
      const span = op.source.type === "bore" ? op.source.centres.length : 1;
      if (i === row) return index;
      index += span;
    }
    return undefined;
  };
  const statusOf = (row: number): "none" | "built" | "warned" | "refused" => {
    if (!result) return "none";
    const first = opIndexOfRow(row);
    if (first === undefined) return "none";
    const op = operations[row];
    if (!op) return "none";
    const span = op.source.type === "bore" ? op.source.centres.length : 1;
    const owns = (i: number | undefined) =>
      i !== undefined && i >= first && i < first + span;
    if (findings.blockers.some((f) => owns(f.opIndex))) return "refused";
    // A refused job with one operation is that operation's refusal, whether or
    // not the finding could be placed: a green tick beside a blocker reads as
    // "all good".
    if (findings.blockers.length > 0 && operations.length === 1) return "refused";
    if (findings.warnings.some((f) => owns(f.opIndex))) return "warned";
    return "built";
  };

  return (
    <div className="space-y-2">
      {operations.map((op, row) => (
        <OperationRow
          key={op.id}
          op={op}
          row={row}
          status={statusOf(row)}
          highlighted={highlighted === row}
          onHighlight={() => setHighlightedOp(highlighted === row ? null : row)}
          onUpdate={(patch) => updateOperation(op.id, patch)}
          onMove={(direction) => moveOperation(op.id, direction)}
          canMoveUp={row > 0}
          canMoveDown={row < operations.length - 1}
        />
      ))}
    </div>
  );
}

interface OperationRowProps {
  op: CamJobOperation;
  row: number;
  status: "none" | "built" | "warned" | "refused";
  highlighted: boolean;
  onHighlight: () => void;
  onUpdate: (patch: Partial<CamJobOperation>) => void;
  onMove: (direction: "up" | "down") => void;
  canMoveUp: boolean;
  canMoveDown: boolean;
}

function OperationRow({
  op,
  status,
  highlighted,
  onHighlight,
  onUpdate,
  onMove,
  canMoveUp,
  canMoveDown,
}: OperationRowProps) {
  const StatusIcon =
    status === "refused" ? XCircle : status === "warned" ? WarningCircle : CheckCircle;
  const statusColour =
    status === "refused"
      ? "text-error"
      : status === "warned"
        ? "text-warning"
        : "text-success";

  return (
    <div
      className={cn(
        "rounded border",
        highlighted ? "border-brand bg-surface-secondary" : "border-border",
      )}
    >
      <div className="flex items-center gap-1 p-2">
        <button
          className="p-1 text-text-muted hover:text-text"
          onClick={() => onUpdate({ enabled: !op.enabled })}
          title={op.enabled ? "Skip this operation" : "Include this operation"}
          aria-label={op.enabled ? "Skip this operation" : "Include this operation"}
        >
          {op.enabled ? <Eye size={14} /> : <EyeSlash size={14} />}
        </button>
        <button
          className={cn("flex-1 text-left text-sm", !op.enabled && "opacity-50")}
          onClick={onHighlight}
          title="Show only this operation in the preview"
        >
          <span className="block truncate">{op.label}</span>
          <span className="block text-xs text-text-muted">
            {KIND_LABELS[op.kind] ?? op.kind}
            {op.source.type === "bore" && op.source.centres.length > 1
              ? ` · ${op.source.centres.length} holes`
              : ""}
          </span>
        </button>
        {status !== "none" && (
          <StatusIcon
            size={14}
            className={statusColour}
            aria-label={
              status === "refused" ? "refused" : status === "warned" ? "warning" : "built"
            }
          />
        )}
        <button
          className="p-1 text-text-muted hover:text-text disabled:opacity-30"
          onClick={() => onMove("up")}
          disabled={!canMoveUp}
          aria-label="Move earlier"
        >
          <ArrowUp size={12} />
        </button>
        <button
          className="p-1 text-text-muted hover:text-text disabled:opacity-30"
          onClick={() => onMove("down")}
          disabled={!canMoveDown}
          aria-label="Move later"
        >
          <ArrowDown size={12} />
        </button>
      </div>

      {highlighted && (
        <div className="px-2 pb-2 space-y-2 border-t border-border pt-2">
          {op.source.type === "opening" && (
            <fieldset className="text-xs space-y-1">
              <legend className="text-text-muted">What happens to the slug</legend>
              <label className="flex items-center gap-2">
                <input
                  type="radio"
                  checked={op.kind === "contour_inside"}
                  onChange={() => onUpdate({ kind: "contour_inside" })}
                />
                Cut out (waste drops free)
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="radio"
                  checked={op.kind === "pocket"}
                  onChange={() => onUpdate({ kind: "pocket" })}
                />
                Pocket (nothing comes loose)
              </label>
            </fieldset>
          )}
          <div className="grid grid-cols-2 gap-2">
            <NumberField
              label="Depth"
              unit="mm"
              value={op.depth}
              min={0}
              onCommit={(depth) => onUpdate({ depth })}
            />
            <NumberField
              label="Stepdown"
              unit="mm"
              value={op.stepdown}
              min={0}
              onCommit={(stepdown) => onUpdate({ stepdown })}
            />
            <NumberField
              label="Feed"
              unit="mm/min"
              step={10}
              value={op.feed}
              min={0}
              onCommit={(feed) => onUpdate({ feed })}
            />
            <NumberField
              label="Plunge"
              unit="mm/min"
              step={10}
              value={op.plunge}
              min={0}
              onCommit={(plunge) => onUpdate({ plunge })}
            />
          </div>
          {op.kind === "contour_outside" && (
            <div className="grid grid-cols-3 gap-2">
              <NumberField
                label="Tabs"
                value={op.tabs ?? 0}
                step={1}
                min={0}
                onCommit={(tabs) => onUpdate({ tabs: Math.round(tabs) })}
                title="Tabs hold the part while the cut that frees it finishes."
              />
              <NumberField
                label="Width"
                unit="mm"
                value={op.tab_width ?? 4}
                min={0}
                onCommit={(tab_width) => onUpdate({ tab_width })}
              />
              <NumberField
                label="Height"
                unit="mm"
                value={op.tab_height ?? 1}
                min={0}
                onCommit={(tab_height) => onUpdate({ tab_height })}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
