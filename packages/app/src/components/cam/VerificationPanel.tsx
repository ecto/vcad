/**
 * What the oracle said, in machinist language.
 *
 * `policy.blocked_by` names checks — `gouge`, `depth`, `loose_pieces`. Nobody
 * standing at a machine wants to read that. So every failed check becomes one
 * sentence built from its own worst violation: what happened, how far, and
 * where. Every check shows a number, pass or fail: "0.000 mm deepest" is a
 * result, "clean" is a mood.
 *
 * Warnings are acknowledged one at a time, and an edit forgets them — a slug
 * that drops free is not a thing to click past in a batch.
 */
import { useMemo } from "react";
import { CheckCircle } from "@phosphor-icons/react/dist/ssr/CheckCircle";
import { WarningCircle } from "@phosphor-icons/react/dist/ssr/WarningCircle";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { Question } from "@phosphor-icons/react/dist/ssr/Question";
import { SealCheck } from "@phosphor-icons/react/dist/ssr/SealCheck";
import { camChecks, type CamCheckReport, type CamVerification } from "@vcad/engine";
import {
  checkTitle,
  findingsOf,
  liveAcknowledgements,
  useCamJobStore,
  verdictOf,
} from "@/stores/cam-job-store";
import { cn } from "@/lib/utils";

const mm = (v: number | undefined, places = 2): string =>
  v === undefined || !Number.isFinite(v) ? "—" : v.toFixed(places);

/**
 * The one number each check is worth showing.
 *
 * Deliberately per-check rather than a generic `worst`: "0.42" beside
 * "Holding tabs" means nothing, and "3 cut, 0 passes step over" is the answer.
 */
function checkValue(check: CamCheckReport, v: CamVerification | undefined): string {
  switch (check.name) {
    case "gouge":
      return `${mm(check.worst, 3)} mm deepest`;
    case "material_left":
      return `${mm(v?.material_left.unswept_area, 1)} mm² left, ${mm(
        v?.material_left.max_standoff,
      )} mm proud`;
    case "rapids":
      return check.pass ? "none below the stock top" : String(check.violation_count);
    case "depth": {
      const skin = v?.depth.remaining_under_part;
      const under =
        skin === undefined
          ? "—"
          : skin >= 0
            ? `${mm(skin, 3)} mm left under`
            : `${mm(-skin, 3)} mm past the underside`;
      return `deepest Z ${mm(v?.depth.deepest_z, 3)}, ${under}`;
    }
    case "tabs":
      return `${v?.tabs.tab_count ?? 0} cut, ${v?.tabs.passes_below_tabs ?? 0} pass(es) step over`;
    case "envelope":
      return v
        ? `X ${mm(v.envelope.work_min[0], 1)}…${mm(v.envelope.work_max[0], 1)}, Y ${mm(
            v.envelope.work_min[1],
            1,
          )}…${mm(v.envelope.work_max[1], 1)}`
        : "—";
    case "loose_pieces":
      return v?.loose.skin_holds
        ? "nothing breaks through"
        : `${v?.loose.pieces.length ?? 0} piece(s) free`;
    case "plunges":
      return check.pass ? "none into uncut metal" : String(check.violation_count);
    default:
      return check.pass ? "clear" : String(check.violation_count);
  }
}

export function VerificationPanel() {
  const result = useCamJobStore((s) => s.result);
  const buildError = useCamJobStore((s) => s.buildError);
  const acknowledge = useCamJobStore((s) => s.acknowledge);
  // Selected as the three primitives it is made of, not as the Set itself: a
  // selector that builds a new object every call never compares equal, and
  // React re-renders until it gives up ("Maximum update depth exceeded").
  const acknowledgedKey = useCamJobStore((s) => s.acknowledgedKey);
  const acknowledgedIds = useCamJobStore((s) => s.acknowledgedIds);
  const builtKey = useCamJobStore((s) => s.builtKey);
  const acknowledged = useMemo(
    () => liveAcknowledgements({ acknowledgedKey, acknowledgedIds, builtKey }),
    [acknowledgedKey, acknowledgedIds, builtKey],
  );
  const setHighlightedOp = useCamJobStore((s) => s.setHighlightedOp);

  const verdict = verdictOf(result);

  if (buildError) {
    return (
      <div className="px-2 py-2 bg-error/10 text-error text-xs rounded">{buildError}</div>
    );
  }
  if (!result) {
    return <p className="text-xs text-text-muted">{verdict.text}</p>;
  }

  const { blockers, warnings } = findingsOf(result);
  const blockedBy = new Set(result.policy?.blocked_by ?? []);
  const checks = camChecks(result.verification);

  const HeadlineIcon =
    verdict.tone === "refused"
      ? XCircle
      : verdict.tone === "clean"
        ? SealCheck
        : Question;
  const headlineColour =
    verdict.tone === "refused"
      ? "text-error"
      : verdict.tone === "clean"
        ? "text-success"
        : "text-text-muted";

  return (
    <div className="space-y-3 text-sm">
      <div className={cn("flex items-center gap-2 font-medium", headlineColour)}>
        <HeadlineIcon size={18} weight="fill" />
        <span>{verdict.text}</span>
      </div>

      {blockers.length > 0 && (
        <section className="space-y-1">
          <h3 className="text-xs uppercase tracking-wide text-text-muted">
            Why it will not run
          </h3>
          {blockers.map((f) => (
            <button
              key={f.id}
              className="w-full flex items-start gap-2 text-left text-xs px-2 py-1.5 rounded bg-error/10 hover:bg-error/20"
              onClick={() => f.opIndex !== undefined && setHighlightedOp(f.opIndex)}
            >
              <XCircle size={14} className="text-error mt-0.5 shrink-0" />
              <span>{f.text}</span>
            </button>
          ))}
        </section>
      )}

      {warnings.length > 0 && (
        <section className="space-y-1">
          <h3 className="text-xs uppercase tracking-wide text-text-muted">
            Read before you run
          </h3>
          {warnings.map((f) => (
            <div
              key={f.id}
              className="flex items-start gap-2 text-xs px-2 py-1.5 rounded bg-warning/10"
            >
              <input
                type="checkbox"
                className="mt-0.5"
                checked={acknowledged.has(f.id)}
                onChange={(e) => acknowledge(f.id, e.target.checked)}
                aria-label={`Acknowledge: ${f.text}`}
              />
              <WarningCircle size={14} className="text-warning mt-0.5 shrink-0" />
              <span>{f.text}</span>
            </div>
          ))}
        </section>
      )}

      {checks.length > 0 && (
        <section className="space-y-1">
          <h3 className="text-xs uppercase tracking-wide text-text-muted">Checks</h3>
          {checks.map((check) => {
            const verdictOfCheck = check.pass
              ? "pass"
              : blockedBy.has(check.name)
                ? "blocked"
                : "warning";
            const Icon =
              verdictOfCheck === "pass"
                ? CheckCircle
                : verdictOfCheck === "blocked"
                  ? XCircle
                  : WarningCircle;
            const colour =
              verdictOfCheck === "pass"
                ? "text-success"
                : verdictOfCheck === "blocked"
                  ? "text-error"
                  : "text-warning";
            return (
              <div
                key={check.name}
                className="flex items-start gap-2 text-xs"
                title={check.note}
              >
                <Icon size={14} className={cn(colour, "mt-0.5 shrink-0")} />
                <span className="flex-1">{checkTitle(check.name)}</span>
                <span className="text-text-muted text-right">
                  {checkValue(check, result.verification)}
                </span>
              </div>
            );
          })}
        </section>
      )}

      {result.notes?.length > 0 && (
        <section className="space-y-1">
          <h3 className="text-xs uppercase tracking-wide text-text-muted">Notes</h3>
          {result.notes.map((n, i) => (
            <p key={i} className="text-xs text-text-muted">
              <span className="uppercase">{String(n.level)}</span>: {n.text}
            </p>
          ))}
        </section>
      )}

      <p className="text-xs text-text-muted">
        Cycle time, feeds and cutter fit are computed, not measured. They stay
        predictions until a part comes off the machine and is measured.
      </p>
    </div>
  );
}
