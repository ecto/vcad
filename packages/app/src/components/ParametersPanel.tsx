import { useEffect, useMemo, useState } from "react";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { CaretLeft } from "@phosphor-icons/react/dist/ssr/CaretLeft";
import { WarningCircle } from "@phosphor-icons/react/dist/ssr/WarningCircle";
import { ChartBar } from "@phosphor-icons/react/dist/ssr/ChartBar";
import type { SensitivityRow } from "@vcad/engine";
import type { Expr, Parameter } from "@vcad/ir";
import {
  mergeParametersIntoDocument,
  useDocumentStore,
  useEngineStore,
  useParametersStore,
  useUiStore,
} from "@vcad/core";
import {
  SENSITIVITY_QUANTITIES,
  documentRevision,
  formatDerivative,
  influenceOf,
  rankedRows,
  trustLabel,
  useSensitivityStore,
} from "@/stores/sensitivity-store";
import {
  ExpressionParseError,
  evaluateExpression,
  expressionFreeVars,
  parseExpression,
  resolveParameters,
} from "@vcad/engine";
import { cn } from "@/lib/utils";
import { Tooltip } from "@/components/ui/tooltip";

function BackButton() {
  const setSidebarPane = useUiStore((s) => s.setSidebarPane);
  return (
    <Tooltip content="Back to tree" side="bottom">
      <button
        onClick={() => setSidebarPane("tree")}
        className="flex h-6 w-6 -ml-1 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
        aria-label="Back to tree"
      >
        <CaretLeft size={14} />
      </button>
    </Tooltip>
  );
}

function SectionHeader({ children }: { children: string }) {
  return (
    <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted pt-2 pb-1">
      {children}
    </div>
  );
}

/** Find a parameter name not already in use. */
function uniqueName(existing: Record<string, Parameter>, base = "p"): string {
  if (!(base in existing)) return base;
  let i = 1;
  while (true) {
    const candidate = `${base}${i}`;
    if (!(candidate in existing)) return candidate;
    i++;
  }
}

/**
 * The influence line under a parameter: what this knob does to the selected
 * quantity, how much of that quantity it commands relative to the other
 * knobs, and how far the answer may be acted on.
 *
 * The bar is `|dJ/dθ| × trust span`, not the raw derivative — a derivative
 * alone ranks by units, not by importance. A knob with a huge slope over a
 * 0.02 mm valid range commands less than a gentle one you can move 10 mm.
 */
function InfluenceLine({
  row,
  max,
  rank,
}: {
  row: SensitivityRow;
  max: number;
  rank: number;
}) {
  const influence = influenceOf(row);
  const fraction = influence != null && max > 0 ? influence / max : 0;
  const trust = trustLabel(row);
  const unverifiable = row.verdict === "unverifiable";

  return (
    <div className="flex flex-col gap-0.5 pt-0.5">
      <div className="flex items-center gap-2">
        <span
          className={cn(
            "text-[10px] tabular-nums w-4 shrink-0",
            rank === 0 ? "text-brand font-medium" : "text-text-muted",
          )}
          aria-hidden
        >
          #{rank + 1}
        </span>
        <div
          className="h-1 flex-1 min-w-0 rounded-full bg-border/60 overflow-hidden"
          role="img"
          aria-label={`Influence ${(fraction * 100).toFixed(0)}% of the strongest parameter`}
        >
          <div
            className={cn(
              "h-full rounded-full transition-[width] duration-200",
              unverifiable ? "bg-text-muted" : "bg-brand",
            )}
            style={{ width: `${Math.max(fraction * 100, influence ? 2 : 0)}%` }}
          />
        </div>
        <span className="text-[10px] font-mono tabular-nums text-text-muted shrink-0">
          {formatDerivative(row)}
        </span>
      </div>
      <div className="flex items-center gap-2 pl-6">
        {unverifiable ? (
          <span className="text-[10px] text-amber-500">
            unverifiable — {row.note ?? "no established derivative"}
          </span>
        ) : (
          <span className="text-[10px] text-text-muted">
            {trust ?? "no trust radius established"}
            {row.route.route === "finite_difference" ? " · finite difference" : ""}
          </span>
        )}
      </div>
    </div>
  );
}

interface ParameterRowProps {
  name: string;
  parameter: Parameter;
  /** Resolved environment (name → value). NaN means its own formula failed. */
  env: Record<string, number>;
  onRename: (oldName: string, newName: string) => void;
  onUpdate: (name: string, parameter: Parameter) => void;
  onRemove: (name: string) => void;
  /** How many bindings reference this parameter. */
  references: number;
  /** Sensitivity row for this parameter, when one has been computed. */
  sensitivity?: { row: SensitivityRow; max: number; rank: number };
}

function ParameterRow({
  name,
  parameter,
  env,
  onRename,
  onUpdate,
  onRemove,
  references,
  sensitivity,
}: ParameterRowProps) {
  const [nameDraft, setNameDraft] = useState(name);
  const [valueDraft, setValueDraft] = useState<string>(String(parameter.value));
  const [error, setError] = useState<string | null>(null);

  const evaluated = env[name];
  const unit = parameter.unit ?? "";

  function commitName() {
    const trimmed = nameDraft.trim();
    if (!trimmed || trimmed === name) {
      setNameDraft(name);
      return;
    }
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(trimmed)) {
      setError(`invalid name '${trimmed}'`);
      setNameDraft(name);
      return;
    }
    setError(null);
    onRename(name, trimmed);
  }

  function commitValue() {
    const trimmed = valueDraft.trim();
    if (!trimmed) {
      setError("value required");
      setValueDraft(String(parameter.value));
      return;
    }
    // Bare number?
    const n = Number(trimmed);
    let next: Expr;
    if (!Number.isNaN(n) && Number.isFinite(n)) {
      next = n;
    } else {
      try {
        parseExpression(trimmed);
        next = trimmed;
      } catch (e) {
        setError(e instanceof ExpressionParseError ? e.message : "invalid expression");
        return;
      }
    }
    setError(null);
    onUpdate(name, { ...parameter, value: next });
  }

  return (
    <div className="flex flex-col gap-1 border-b border-border/40 py-2">
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={nameDraft}
          onChange={(e) => setNameDraft(e.target.value)}
          onBlur={commitName}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") {
              setNameDraft(name);
              e.currentTarget.blur();
            }
          }}
          spellCheck={false}
          className="flex-1 min-w-0 rounded-md bg-card border border-border text-text text-xs font-mono outline-none px-2 py-1 focus:border-brand focus:ring-2 focus:ring-brand/30 transition-colors"
          aria-label="Parameter name"
        />
        <button
          type="button"
          onClick={() => {
            if (references > 0) {
              const ok = confirm(
                `${name} is referenced by ${references} binding${
                  references === 1 ? "" : "s"
                }. Remove anyway?`,
              );
              if (!ok) return;
            }
            onRemove(name);
          }}
          className="flex h-6 w-6 shrink-0 items-center justify-center text-text-muted hover:text-red-500 hover:bg-hover"
          aria-label={`Remove ${name}`}
          title={`Remove ${name}`}
        >
          <Trash size={12} />
        </button>
      </div>
      <div className="flex items-center gap-2">
        <input
          type="text"
          value={valueDraft}
          onChange={(e) => {
            setValueDraft(e.target.value);
            setError(null);
          }}
          onBlur={commitValue}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") {
              setValueDraft(String(parameter.value));
              e.currentTarget.blur();
            }
          }}
          spellCheck={false}
          className={cn(
            "flex-1 min-w-0 rounded-md bg-card border text-text text-xs font-mono outline-none px-2 py-1 focus:ring-2 focus:ring-brand/30 transition-colors",
            error ? "border-red-500" : "border-border focus:border-brand",
          )}
          aria-label={`${name} value`}
        />
        <span
          className="text-[10px] text-text-muted tabular-nums min-w-[4rem] text-right"
          title={error ?? undefined}
        >
          {error
            ? "!"
            : evaluated != null && Number.isFinite(evaluated)
              ? `= ${round(evaluated, 3)}${unit ? ` ${unit}` : ""}`
              : "= ?"}
        </span>
      </div>
      {error && (
        <div className="flex items-center gap-1 text-[10px] text-red-500">
          <WarningCircle size={10} />
          <span>{error}</span>
        </div>
      )}
      {references > 0 && (
        <div className="text-[10px] text-text-muted">
          referenced by {references} field{references === 1 ? "" : "s"}
        </div>
      )}
      {sensitivity && (
        <InfluenceLine
          row={sensitivity.row}
          max={sensitivity.max}
          rank={sensitivity.rank}
        />
      )}
    </div>
  );
}

/**
 * The influence control: pick a quantity, compute, read the ranking.
 *
 * Deliberately a button rather than something that recomputes as you type.
 * The sweep costs one seam pass per parameter plus a topology search — much
 * cheaper than the rebuild-N-times any other CAD tool would need to answer
 * the same question, and much too expensive to run on every keystroke.
 */
function InfluenceControls() {
  const rawDocument = useDocumentStore((s) => s.document);
  const parameters = useParametersStore((s) => s.parameters);
  const bindings = useParametersStore((s) => s.bindings);
  const engine = useEngineStore((s) => s.engine);

  // Named parameters and their bindings live in their own store; the
  // document only carries them once merged. Differentiating the raw
  // document would silently see zero parameters and report nothing —
  // the same merge every `engine.evaluate` call does.
  const document = useMemo(
    () => mergeParametersIntoDocument(rawDocument, parameters, bindings),
    [rawDocument, parameters, bindings],
  );
  const { report, loading, error, quantity, computedFor } = useSensitivityStore();
  const setQuantity = useSensitivityStore((s) => s.setQuantity);
  const compute = useSensitivityStore((s) => s.compute);
  const invalidate = useSensitivityStore((s) => s.invalidate);

  // A revision key that changes whenever the geometry or the parameter values
  // do, so a stale gradient never sits next to a changed model.
  const revision = useMemo(() => documentRevision(document), [document]);

  useEffect(() => {
    if (computedFor !== null && computedFor !== revision) invalidate();
  }, [revision, computedFor, invalidate]);

  const stale = computedFor !== null && computedFor !== revision;

  return (
    <div className="flex flex-col gap-1 pb-1">
      <div className="flex items-center gap-2">
        <select
          value={quantity}
          onChange={(e) => setQuantity(e.target.value)}
          className="flex-1 min-w-0 rounded-md bg-card border border-border text-text text-[11px] font-mono outline-none px-2 py-1 focus:border-brand"
          aria-label="Quantity to rank parameters by"
        >
          {SENSITIVITY_QUANTITIES.map((q) => (
            <option key={q} value={q}>
              d({q})/dθ
            </option>
          ))}
        </select>
        <button
          type="button"
          disabled={!engine || loading}
          onClick={() => engine && compute(document, engine, revision)}
          className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-text-muted hover:text-text hover:bg-hover disabled:opacity-40"
        >
          <ChartBar size={12} />
          {loading ? "Solving…" : report && !stale ? "Recompute" : "Rank"}
        </button>
      </div>
      {error && (
        <div className="flex items-start gap-1 text-[10px] text-red-500">
          <WarningCircle size={10} className="mt-0.5 shrink-0" />
          <span>{error}</span>
        </div>
      )}
      {!report && !loading && !error && (
        <div className="text-[10px] text-text-muted">
          Rank every parameter by how much it moves this quantity — one
          gradient pass, not a rebuild per knob.
        </div>
      )}
      {report && !report.allUsable && (
        <div className="text-[10px] text-amber-500">
          {report.unusable.length} row(s) could not be established and must not
          steer a change.
        </div>
      )}
    </div>
  );
}

export function ParametersPanel() {
  const parameters = useParametersStore((s) => s.parameters);
  const bindings = useParametersStore((s) => s.bindings);
  const setParameter = useParametersStore((s) => s.setParameter);
  const renameParameter = useParametersStore((s) => s.renameParameter);
  const removeParameter = useParametersStore((s) => s.removeParameter);

  // Resolve parameters into an env; on cycle / parse error, fall back to
  // a safe per-parameter resolution so good parameters still display.
  const env = useMemo(() => {
    try {
      return resolveParameters(parameters);
    } catch {
      const out: Record<string, number> = {};
      for (const [name, p] of Object.entries(parameters)) {
        try {
          out[name] =
            typeof p.value === "number" ? p.value : evaluateExpression(p.value, out);
        } catch {
          out[name] = Number.NaN;
        }
      }
      return out;
    }
  }, [parameters]);

  // Count how many bindings reference each parameter.
  const referenceCounts = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const expr of Object.values(bindings)) {
      if (typeof expr !== "string") continue;
      try {
        const ast = parseExpression(expr);
        for (const v of expressionFreeVars(ast)) {
          counts[v] = (counts[v] ?? 0) + 1;
        }
      } catch {
        // skip malformed
      }
    }
    return counts;
  }, [bindings]);

  const report = useSensitivityStore((s) => s.report);
  const quantity = useSensitivityStore((s) => s.quantity);

  // Rank once, then look each parameter's row up by name. Parameters the
  // sweep could not price simply have no influence line.
  const sensitivityByName = useMemo(() => {
    const { rows, max } = rankedRows(report, quantity);
    const out = new Map<string, { row: SensitivityRow; max: number; rank: number }>();
    rows.forEach((row, rank) => out.set(row.parameter, { row, max, rank }));
    return out;
  }, [report, quantity]);

  // With a ranking in hand, sort the panel by influence — the whole point is
  // to put the knob that matters at the top. Without one, sort by name.
  const names = useMemo(() => {
    const all = Object.keys(parameters);
    if (sensitivityByName.size === 0) return all.sort();
    return all.sort((a, b) => {
      const ra = sensitivityByName.get(a)?.rank ?? Number.MAX_SAFE_INTEGER;
      const rb = sensitivityByName.get(b)?.rank ?? Number.MAX_SAFE_INTEGER;
      return ra === rb ? a.localeCompare(b) : ra - rb;
    });
  }, [parameters, sensitivityByName]);

  function addParameter() {
    const name = uniqueName(parameters, "p");
    setParameter(name, { value: 0 });
  }

  return (
    <div className="flex h-full w-full flex-col overflow-hidden">
      <div className="flex items-center gap-1 border-b border-border px-2 py-1">
        <BackButton />
        <span className="flex-1 text-xs font-medium uppercase tracking-wider text-text-muted">
          Parameters
        </span>
        <Tooltip content="Add parameter" side="bottom">
          <button
            type="button"
            onClick={addParameter}
            className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
            aria-label="Add parameter"
          >
            <Plus size={14} />
          </button>
        </Tooltip>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto px-2 pb-4">
        {names.length === 0 ? (
          <div className="pt-6 text-center text-xs text-text-muted">
            No parameters yet.
            <br />
            <button
              type="button"
              onClick={addParameter}
              className="mt-2 text-brand hover:underline"
            >
              Add your first parameter
            </button>
            <div className="mt-6 text-[10px] opacity-75 px-4">
              Parameters can be referenced by any numeric field.
              Try: <span className="font-mono">wheelbase</span>,{" "}
              <span className="font-mono">headAngle</span>, or a derived
              value like <span className="font-mono">wheelbase * 0.5</span>.
            </div>
          </div>
        ) : (
          <>
            <SectionHeader>Influence</SectionHeader>
            <InfluenceControls />
            <SectionHeader>Definitions</SectionHeader>
            {names.map((name) => (
              <ParameterRow
                key={name}
                name={name}
                parameter={parameters[name]!}
                env={env}
                onRename={renameParameter}
                onUpdate={setParameter}
                onRemove={removeParameter}
                references={referenceCounts[name] ?? 0}
                sensitivity={sensitivityByName.get(name)}
              />
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function round(n: number, decimals = 3): number {
  const f = Math.pow(10, decimals);
  return Math.round(n * f) / f;
}
