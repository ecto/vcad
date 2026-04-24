import { useMemo, useState } from "react";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { CaretLeft } from "@phosphor-icons/react/dist/ssr/CaretLeft";
import { WarningCircle } from "@phosphor-icons/react/dist/ssr/WarningCircle";
import type { Expr, Parameter } from "@vcad/ir";
import { useParametersStore, useUiStore } from "@vcad/core";
import {
  ExpressionParseError,
  evaluateExpression,
  expressionFreeVars,
  parseExpression,
  resolveParameters,
} from "@vcad/engine";
import { cn } from "@/lib/utils";

function BackButton() {
  const setSidebarPane = useUiStore((s) => s.setSidebarPane);
  return (
    <button
      onClick={() => setSidebarPane("tree")}
      className="flex h-6 w-6 -ml-1 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
      aria-label="Back to tree"
      title="Back to tree"
    >
      <CaretLeft size={14} />
    </button>
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
}

function ParameterRow({
  name,
  parameter,
  env,
  onRename,
  onUpdate,
  onRemove,
  references,
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
          className="flex-1 min-w-0 bg-card border border-border text-text text-xs font-mono outline-none px-2 py-1 focus:border-brand"
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
            "flex-1 min-w-0 bg-card border text-text text-xs font-mono outline-none px-2 py-1 focus:border-brand",
            error ? "border-red-500" : "border-border",
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

  const names = Object.keys(parameters).sort();

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
        <button
          type="button"
          onClick={addParameter}
          className="flex h-6 w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          aria-label="Add parameter"
          title="Add parameter"
        >
          <Plus size={14} />
        </button>
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
